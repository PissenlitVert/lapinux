//! DepotDownloader, le téléchargeur d'atelier sans client Steam.
//!
//! L'outil est récupéré depuis les releases officielles de SteamRE/DepotDownloader
//! et conservé dans le dossier de données de Lapinux. Il est autonome : aucune
//! installation .NET n'est demandée. Le binaire est lancé dans un `HOME` privé,
//! sans affichage. Quand une session Lapinux existe, son jeton de refresh est
//! copié dans le stockage isolé de DepotDownloader le temps du téléchargement :
//! aucun mot de passe n'est demandé et la copie disparaît avec le `HOME`.

use crate::session;
use lapinux_core::i18n::{t, t_args};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Version dont les binaires Linux sont épinglés.
pub const VERSION: &str = "3.4.0";
const RELEASE: &str = "DepotDownloader_3.4.0";
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// Racine du téléchargeur et de ses fichiers de travail.
pub fn root(data_dir: &Path) -> PathBuf {
    data_dir.join("depotdownloader")
}

/// Dossier des contenus en cours de téléchargement.
pub fn work(data_dir: &Path) -> PathBuf {
    root(data_dir).join("work")
}

/// Journal lisible par l'interface après un échec.
pub fn log_path(data_dir: &Path, appid: u32) -> PathBuf {
    root(data_dir).join("logs").join(format!("{appid}.log"))
}

/// Prépare un `HOME` temporaire pour DepotDownloader. Si une session Lapinux
/// est enregistrée, son jeton de refresh est copié dans le stockage isolé de
/// l'outil ; la fonction rend alors le nom du compte. Sans session, le
/// téléchargement reste anonyme et aucun fichier de compte n'est écrit.
///
/// Le `HOME` doit être supprimé par l'appelant après le téléchargement : il
/// contient une copie du jeton, jamais le mot de passe.
pub fn prepare_home(binary: &Path, data_dir: &Path, home: &Path) -> Result<Option<String>, String> {
    let _ = fs::remove_dir_all(home);
    fs::create_dir_all(home)
        .map_err(|error| t_args("downloads.depot_home_prepare", &[("error", &error.to_string())]))?;
    fs::set_permissions(home, fs::Permissions::from_mode(0o700))
        .map_err(|error| t_args("downloads.depot_home_permissions", &[("error", &error.to_string())]))?;

    let Some(saved) = session::load_from(&session::path_in(data_dir)) else {
        return Ok(None);
    };
    let account = saved.account_name.trim().to_string();
    let token = saved.refresh_token.trim();
    if account.is_empty() || token.is_empty() {
        return Ok(None);
    }

    // `-h` initialise le stockage isolé .NET avant la vraie connexion. Son
    // code de sortie n'importe pas : seul le dossier `AssemFiles` compte.
    let _ = Command::new(binary)
        .arg("-h")
        .current_dir(home)
        .env("HOME", home)
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    let directories = assem_files(&home.join(".local/share/IsolatedStorage"));
    if directories.is_empty() {
        return Err(t("downloads.depot_storage_not_initialized").into());
    }
    let encoded = account_config(&account, token)?;
    for directory in directories {
        write_private(&directory.join("account.config"), &encoded)?;
    }
    Ok(Some(account))
}

/// Dossiers où DepotDownloader range son `account.config`.
fn assem_files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0_usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if !kind.is_dir() {
                continue;
            }
            let path = entry.path();
            if entry.file_name().to_string_lossy() == "AssemFiles" {
                found.push(path);
            } else if depth < 8 {
                stack.push((path, depth + 1));
            }
        }
    }
    found
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| t_args("downloads.depot_session_write", &[("error", &error.to_string())]))?;
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|error| t_args("downloads.depot_session_permissions", &[("error", &error.to_string())]))?;
    file.write_all(bytes)
        .map_err(|error| t_args("downloads.depot_session_write", &[("error", &error.to_string())]))?;
    file.sync_all()
        .map_err(|error| t_args("downloads.depot_session_sync", &[("error", &error.to_string())]))?;
    Ok(())
}

/// Format protobuf compressé du `account.config` de DepotDownloader :
/// `LoginTokens` est le champ 4, une map dont chaque entrée porte le nom du
/// compte (champ 1) et son jeton (champ 2).
fn account_config(account: &str, token: &str) -> Result<Vec<u8>, String> {
    let mut entry = Vec::new();
    put_bytes(&mut entry, 1, account.as_bytes());
    put_bytes(&mut entry, 2, token.as_bytes());
    let mut message = Vec::new();
    put_bytes(&mut message, 4, &entry);

    let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
    encoder
        .write_all(&message)
        .map_err(|error| t_args("downloads.depot_session_compress", &[("error", &error.to_string())]))?;
    encoder
        .finish()
        .map_err(|error| t_args("downloads.depot_session_compress", &[("error", &error.to_string())]))
}

fn put_bytes(out: &mut Vec<u8>, field: u32, bytes: &[u8]) {
    put_varint(out, (u64::from(field) << 3) | 2);
    put_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// Nom et URL de l'archive correspondant à l'architecture de la machine.
fn asset() -> Result<(&'static str, String), String> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => {
            return Err(t_args("downloads.depot_unsupported_arch", &[("arch", other)]));
        }
    };
    Ok((
        arch,
        format!(
            "https://github.com/SteamRE/DepotDownloader/releases/download/{RELEASE}/DepotDownloader-linux-{arch}.zip"
        ),
    ))
}

/// Rend le binaire disponible localement, en le téléchargeant une seule fois.
pub fn ensure(data_dir: &Path) -> Result<PathBuf, String> {
    let (arch, url) = asset()?;
    let root = root(data_dir);
    let version = root.join(VERSION);
    let executable = version.join("DepotDownloader");
    if executable.is_file() {
        return Ok(executable);
    }

    fs::create_dir_all(&version)
        .map_err(|error| t_args("downloads.depot_dir_create", &[("error", &error.to_string())]))?;
    let archive = root.join(format!("DepotDownloader-{VERSION}-linux-{arch}.zip"));
    if !archive.is_file() {
        download(&url, &archive)?;
    }
    extract(&archive, &executable)?;
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))
        .map_err(|error| format!("droits de DepotDownloader : {error}"))?;
    if !executable.is_file() {
        return Err(t_args("downloads.depot_missing_extract", &[("path", &executable.display().to_string())]));
    }
    Ok(executable)
}

fn download(url: &str, archive: &Path) -> Result<(), String> {
    let part = archive.with_extension("zip.part");
    let _ = fs::remove_file(&part);
    let agent = ureq::AgentBuilder::new().timeout(DOWNLOAD_TIMEOUT).build();
    let response = agent
        .get(url)
        .set("User-Agent", "Lapinux/0.1")
        .call()
        .map_err(|error| t_args("downloads.depot_download", &[("error", &error.to_string())]))?;
    let mut reader = response.into_reader();
    let mut file =
        File::create(&part).map_err(|error| t_args("downloads.depot_archive", &[("error", &error.to_string())]))?;
    io::copy(&mut reader, &mut file)
        .map_err(|error| t_args("downloads.depot_write", &[("error", &error.to_string())]))?;
    file.sync_all()
        .map_err(|error| t_args("downloads.depot_sync", &[("error", &error.to_string())]))?;
    drop(file);
    fs::rename(&part, archive).map_err(|error| t_args("downloads.depot_archive", &[("error", &error.to_string())]))?;
    Ok(())
}

fn extract(archive: &Path, executable: &Path) -> Result<(), String> {
    let file = File::open(archive)
        .map_err(|error| t_args("downloads.depot_archive_open", &[("error", &error.to_string())]))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|error| t_args("downloads.depot_archive_read", &[("error", &error.to_string())]))?;
    let mut entry = zip
        .by_name("DepotDownloader")
        .map_err(|error| t_args("downloads.depot_binary_missing", &[("error", &error.to_string())]))?;
    if entry.is_dir() {
        return Err(t("downloads.depot_archive_no_binary").into());
    }
    let part = executable.with_extension("part");
    let _ = fs::remove_file(&part);
    let mut out =
        File::create(&part).map_err(|error| t_args("downloads.depot_write", &[("error", &error.to_string())]))?;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = entry
            .read(&mut buffer)
            .map_err(|error| t_args("downloads.depot_read", &[("error", &error.to_string())]))?;
        if read == 0 {
            break;
        }
        out.write_all(&buffer[..read])
            .map_err(|error| t_args("downloads.depot_write", &[("error", &error.to_string())]))?;
    }
    out.sync_all()
        .map_err(|error| t_args("downloads.depot_sync", &[("error", &error.to_string())]))?;
    drop(out);
    fs::rename(&part, executable)
        .map_err(|error| t_args("downloads.depot_install", &[("error", &error.to_string())]))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_compte_est_encode_dans_le_format_de_depotdownloader() {
        let encoded = account_config("alice", "refresh-token").expect("compte encodé");
        let mut decoder = flate2::read::DeflateDecoder::new(encoded.as_slice());
        let mut raw = Vec::new();
        decoder.read_to_end(&mut raw).expect("flux relu");
        let text = String::from_utf8_lossy(&raw);
        assert!(text.contains("alice"));
        assert!(text.contains("refresh-token"));
        assert_eq!(raw.first(), Some(&0x22), "champ 4 de LoginTokens");
    }

    #[test]
    fn les_dossiers_de_session_sont_trouves_sous_isolated_storage() {
        let dir = tempfile::tempdir().unwrap();
        let assem = dir.path().join("IsolatedStorage/a/b/Url.x/AssemFiles");
        fs::create_dir_all(&assem).unwrap();
        assert_eq!(
            assem_files(&dir.path().join("IsolatedStorage")),
            vec![assem]
        );
    }

    #[test]
    fn prepare_home_ecrit_le_jeton_sans_mot_de_passe() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let home = dir.path().join("home");
        fs::create_dir_all(&data).unwrap();
        session::save_to(
            &session::path_in(&data),
            &session::SavedSession {
                account_name: "alice".into(),
                steam_id: 7,
                refresh_token: "refresh".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let binary = dir.path().join("DepotDownloader");
        fs::write(
            &binary,
            "#!/bin/sh\nmkdir -p \"$HOME/.local/share/IsolatedStorage/a/b/AssemFiles\"\n",
        )
        .unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(
            prepare_home(&binary, &data, &home).unwrap(),
            Some("alice".into())
        );
        let config = assem_files(&home.join(".local/share/IsolatedStorage"))
            .pop()
            .expect("dossier de session")
            .join("account.config");
        assert!(config.is_file());
        assert_eq!(
            fs::metadata(&config).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn sans_session_aucun_compte_n_est_prepare() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let home = dir.path().join("home");
        assert_eq!(
            prepare_home(Path::new("/bin/true"), &data, &home).unwrap(),
            None
        );
        assert!(
            home.is_dir(),
            "le HOME privé reste disponible pour l'anonyme"
        );
        assert!(!home.join(".local/share/IsolatedStorage").exists());
    }
}
