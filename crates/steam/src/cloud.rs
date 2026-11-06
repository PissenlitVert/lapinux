//! Sauvegardes Steam Cloud d'un jeu, lues par le protocole.
//!
//! Le service `Cloud` publie ce que le compte garde pour un jeu :
//! `GetAppFileChangelist#1` rend la liste (nom, taille, date, empreinte) et les
//! dossiers que le jeu déclare, `ClientGetAppQuotaUsage#1` le quota consommé.
//! Aucune clé Web API : la session CM du compte suffit, comme pour les succès
//! et le temps de jeu.

use crate::cm::proto::{field_bytes, field_varint, Field, Reader};
use crate::Error;
use lapinux_core::model::{CloudFile, CloudSaves};
use std::time::Duration;
use steamroom::client::{LoggedIn, SteamClient};

/// Liste des fichiers cloud et dossiers déclarés par le jeu.
const FILES_METHOD: &str = "Cloud.GetAppFileChangelist#1";
/// Fichiers et octets utilisés sur le quota du jeu.
const QUOTA_METHOD: &str = "Cloud.ClientGetAppQuotaUsage#1";
/// Adresse de téléchargement d'un fichier gardé dans le cloud.
const DOWNLOAD_METHOD: &str = "Cloud.ClientFileDownload#1";
/// Ouverture d'un lot d'envoi : les fichiers annoncés sont ensuite téléversés.
const BEGIN_BATCH_METHOD: &str = "Cloud.BeginAppUploadBatch#1";
/// Adresses des blocs à envoyer pour un fichier.
const BEGIN_UPLOAD_METHOD: &str = "Cloud.ClientBeginFileUpload#1";
/// Validation d'un fichier téléversé.
const COMMIT_UPLOAD_METHOD: &str = "Cloud.ClientCommitFileUpload#1";
/// Clôture du lot : Steam le publie alors pour le compte.
const COMPLETE_BATCH_METHOD: &str = "Cloud.CompleteAppUploadBatchBlocking#1";
/// Un CM qui ne répond pas en un quart de minute est perdu.
const REPLY_TIMEOUT: Duration = Duration::from_secs(20);
/// Un fichier de sauvegarde reste petit : au-delà, le transfert est coupé.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// Au-delà, le fichier n'est plus une sauvegarde mais un cache : il n'est pas
/// téléversé après une partie.
pub const MAX_UPLOAD: u64 = 64 * 1024 * 1024;

/// État cloud d'un jeu : la liste des fichiers, puis le quota consommé.
///
/// Le quota est une seconde demande : s'il échoue, la liste reste utile et
/// l'erreur est ignorée.
pub async fn game_cloud(client: &SteamClient<LoggedIn>, appid: u32) -> Result<CloudSaves, Error> {
    let mut body = Vec::new();
    field_varint(1, u64::from(appid), &mut body);
    // 0 : tout l'historique, pas seulement ce qui a changé depuis un numéro.
    field_varint(2, 0, &mut body);
    let answer = tokio::time::timeout(
        REPLY_TIMEOUT,
        client.call_service_method(FILES_METHOD, &body),
    )
    .await
    .map_err(|_| Error::Other("Steam n'a pas répondu".into()))?
    .map_err(|error| Error::Other(error.to_string()))?;
    let mut state = parse_changelist(&answer.body);

    let mut quota = Vec::new();
    field_varint(1, u64::from(appid), &mut quota);
    if let Ok(Ok(answer)) = tokio::time::timeout(
        REPLY_TIMEOUT,
        client.call_service_method(QUOTA_METHOD, &quota),
    )
    .await
    {
        parse_quota(&answer.body, &mut state);
    }
    Ok(state)
}

/// Réponse de `GetAppFileChangelist#1` : numéro de changement (1), fichiers
/// (2), dossiers déclarés (4) et noms de machines (5, inutilisés ici).
fn parse_changelist(body: &[u8]) -> CloudSaves {
    let mut state = CloudSaves::default();
    let mut reader = Reader::new(body);
    while let Some((number, field)) = reader.next() {
        match (number, field) {
            (1, Field::Varint(value)) => state.change_number = value,
            (2, Field::Bytes(entry)) => {
                if let Some(file) = parse_file(entry) {
                    state.files.push(file);
                }
            }
            (4, Field::Bytes(prefix)) => {
                if let Ok(text) = std::str::from_utf8(prefix) {
                    if !text.is_empty() {
                        state.prefixes.push(text.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    // Chaque fichier nomme son dossier par son index (7) : la liste des
    // préfixes n'arrive qu'après, elle est donc appliquée ici.
    for file in &mut state.files {
        if let Some(prefix) = state.prefixes.get(file.prefix_index) {
            file.prefix = prefix.clone();
        }
    }
    state
}

/// Une entrée `CCloud_AppFileInfo`.
fn parse_file(entry: &[u8]) -> Option<CloudFile> {
    let mut file = CloudFile::default();
    let mut reader = Reader::new(entry);
    while let Some((number, field)) = reader.next() {
        match (number, field) {
            (1, Field::Bytes(name)) => {
                file.name = String::from_utf8_lossy(name).into_owned();
            }
            (2, Field::Bytes(sha)) => file.sha = sha.to_vec(),
            (3, Field::Varint(value)) => file.modified = value as i64,
            (4, Field::Varint(value)) => file.size = value,
            (7, Field::Varint(value)) => file.prefix_index = value as usize,
            _ => {}
        }
    }
    // Un fichier supprimé reste dans l'historique avec un nom vide : il n'a
    // rien à montrer.
    (!file.name.trim().is_empty()).then_some(file)
}

/// Ce que `ClientFileDownload#1` répond : où prendre le fichier et s'il est
/// chiffré pour ce client.
#[derive(Debug, Default, PartialEq, Eq)]
struct Download {
    /// Octets transférés (le fichier compressé, quand il l'est).
    size: u64,
    /// Octets du fichier une fois décompressé (0 si Steam ne le dit pas).
    raw_size: u64,
    host: String,
    path: String,
    https: bool,
    encrypted: bool,
}

/// Adresse du fichier, ou l'erreur qui explique pourquoi il n'y en a pas.
fn parse_download(body: &[u8]) -> Option<Download> {
    let mut download = Download::default();
    let mut reader = Reader::new(body);
    while let Some((number, field)) = reader.next() {
        match (number, field) {
            (2, Field::Varint(value)) => download.size = value,
            (3, Field::Varint(value)) => download.raw_size = value,
            (7, Field::Bytes(text)) => download.host = String::from_utf8_lossy(text).into_owned(),
            (8, Field::Bytes(text)) => download.path = String::from_utf8_lossy(text).into_owned(),
            (9, Field::Varint(value)) => download.https = value != 0,
            (11, Field::Varint(value)) => download.encrypted = value != 0,
            _ => {}
        }
    }
    (!download.host.trim().is_empty() && !download.path.trim().is_empty()).then_some(download)
}

/// Télécharge un fichier du cloud pour le compte connecté.
///
/// Le CM ne transporte pas les octets : il rend l'adresse du CDN, que ce code
/// va chercher. Un fichier chiffré par le client Steam n'est pas décodable
/// ici — la clé appartient au client —, et l'erreur le dit plutôt que d'écrire
/// un fichier illisible à la place d'une sauvegarde.
pub async fn download_file(
    client: &SteamClient<LoggedIn>,
    appid: u32,
    file: &CloudFile,
) -> Result<Vec<u8>, Error> {
    // Le nom attendu par le service est le chemin complet, dossier déclaré
    // compris : « GameSave.sav » seul répond `FileNotFound`.
    let filename = format!("{}{}", file.prefix, file.name);
    let filename = filename.as_str();
    let mut body = Vec::new();
    field_varint(1, u64::from(appid), &mut body);
    field_bytes(2, filename.as_bytes(), &mut body);
    let answer = tokio::time::timeout(
        REPLY_TIMEOUT,
        client.call_service_method(DOWNLOAD_METHOD, &body),
    )
    .await
    .map_err(|_| Error::Other("Steam n'a pas répondu".into()))?
    .map_err(|error| Error::Other(error.to_string()))?;
    let download = parse_download(&answer.body).ok_or_else(|| {
        Error::Other(format!("aucune adresse pour {filename}"))
    })?;
    if download.encrypted {
        return Err(Error::Other(format!(
            "{filename} est chiffré par le client Steam : Lapinux ne peut pas l'ouvrir"
        )));
    }
    let scheme = if download.https { "https" } else { "http" };
    let url = format!("{scheme}://{}{}", download.host, download.path);
    let http = reqwest::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .map_err(|error| Error::Http(error.to_string()))?;
    let response = http
        .get(&url)
        .send()
        .await
        .map_err(|error| Error::Http(format!("{filename} : {error}")))?;
    if !response.status().is_success() {
        return Err(Error::Http(format!(
            "{filename} : le CDN a répondu {}",
            response.status()
        )));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| Error::Http(format!("{filename} : {error}")))?;
    if download.size > 0 && bytes.len() as u64 != download.size {
        return Err(Error::Http(format!(
            "{filename} : {} octets reçus sur {}",
            bytes.len(),
            download.size
        )));
    }
    // Un fichier que Steam a compressé arrive dans une archive : elle contient
    // la sauvegarde telle que le jeu l'a écrite.
    let bytes = if bytes.starts_with(b"PK\x03\x04") {
        unzip(&bytes, filename)?
    } else {
        bytes.to_vec()
    };
    if download.raw_size > 0 && bytes.len() as u64 != download.raw_size {
        return Err(Error::Other(format!(
            "{filename} : {} octets après décompression sur {}",
            bytes.len(),
            download.raw_size
        )));
    }
    Ok(bytes)
}

/// Contenu de la première entrée d'une archive cloud.
fn unzip(bytes: &[u8], filename: &str) -> Result<Vec<u8>, Error> {
    use std::io::Read;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|error| Error::Other(format!("{filename} : archive illisible ({error})")))?;
    let mut entry = archive
        .by_index(0)
        .map_err(|error| Error::Other(format!("{filename} : archive vide ({error})")))?;
    let mut out = Vec::with_capacity(entry.size() as usize);
    entry
        .read_to_end(&mut out)
        .map_err(|error| Error::Other(format!("{filename} : lecture de l'archive ({error})")))?;
    Ok(out)
}

/// Empreinte SHA-1 d'un fichier, telle que Steam la compare dans ses listes.
pub fn file_sha(data: &[u8]) -> Vec<u8> {
    use sha1::{Digest, Sha1};
    Sha1::digest(data).to_vec()
}

/// Lot d'envoi ouvert sur le serveur : Steam attend ses fichiers, puis sa
/// clôture.
pub async fn begin_batch(
    client: &SteamClient<LoggedIn>,
    appid: u32,
    machine: &str,
    files: &[String],
) -> Result<u64, Error> {
    let mut body = Vec::new();
    field_varint(1, u64::from(appid), &mut body);
    field_bytes(2, machine.as_bytes(), &mut body);
    for file in files {
        field_bytes(3, file.as_bytes(), &mut body);
    }
    let answer = tokio::time::timeout(
        REPLY_TIMEOUT,
        client.call_service_method(BEGIN_BATCH_METHOD, &body),
    )
    .await
    .map_err(|_| Error::Other("Steam n'a pas répondu".into()))?
    .map_err(|error| Error::Other(error.to_string()))?;
    let mut batch = 0u64;
    let mut reader = Reader::new(&answer.body);
    while let Some((number, field)) = reader.next() {
        if let (1, Field::Varint(value)) = (number, field) {
            batch = value;
        }
    }
    if batch == 0 {
        return Err(Error::Other("aucun lot d'envoi ouvert".into()));
    }
    Ok(batch)
}

/// Envoie un fichier et le valide dans le lot ouvert.
///
/// Le fichier part tel quel (Steam accepte un envoi non compressé), en blocs
/// dont le service donne l'adresse : le CM ne transporte pas les octets.
pub async fn upload_file(
    client: &SteamClient<LoggedIn>,
    appid: u32,
    file: &CloudFile,
    data: &[u8],
    batch: u64,
) -> Result<(), Error> {
    let filename = format!("{}{}", file.prefix, file.name);
    let sha = file_sha(data);
    let mut body = Vec::new();
    field_varint(1, u64::from(appid), &mut body);
    field_varint(2, data.len() as u64, &mut body);
    field_varint(3, data.len() as u64, &mut body);
    field_bytes(4, &sha, &mut body);
    field_varint(
        5,
        lapinux_core::util::now_unix().max(0) as u64,
        &mut body,
    );
    field_bytes(6, filename.as_bytes(), &mut body);
    // Toutes plateformes : la sauvegarde reste celle du compte, quel que soit
    // le système d'où le jeu tourne.
    field_varint(7, u64::from(u32::MAX), &mut body);
    // Pas de chiffrement : Lapinux n'a pas la clé du client Steam.
    field_varint(10, 0, &mut body);
    field_varint(13, batch, &mut body);
    let answer = tokio::time::timeout(
        REPLY_TIMEOUT,
        client.call_service_method(BEGIN_UPLOAD_METHOD, &body),
    )
    .await
    .map_err(|_| Error::Other("Steam n'a pas répondu".into()))?
    .map_err(|error| Error::Other(error.to_string()))?;
    let blocks = parse_blocks(&answer.body);
    if blocks.encrypted {
        return Err(Error::Other(format!(
            "{filename} : Steam demande un envoi chiffré, impossible sans le client"
        )));
    }
    if blocks.requests.is_empty() {
        return Err(Error::Other(format!("{filename} : aucune adresse d'envoi")));
    }
    for block in &blocks.requests {
        send_block(block, data).await?;
    }
    let mut commit = Vec::new();
    field_varint(1, 1, &mut commit);
    field_varint(2, u64::from(appid), &mut commit);
    field_bytes(3, &sha, &mut commit);
    field_bytes(4, filename.as_bytes(), &mut commit);
    tokio::time::timeout(
        REPLY_TIMEOUT,
        client.call_service_method(COMMIT_UPLOAD_METHOD, &commit),
    )
    .await
    .map_err(|_| Error::Other("Steam n'a pas répondu".into()))?
    .map_err(|error| Error::Other(format!("{filename} : validation refusée ({error})")))?;
    Ok(())
}

/// Clôt le lot : sans cette clôture, Steam ne publie aucun des fichiers.
pub async fn complete_batch(
    client: &SteamClient<LoggedIn>,
    appid: u32,
    batch: u64,
) -> Result<(), Error> {
    let mut body = Vec::new();
    field_varint(1, u64::from(appid), &mut body);
    field_varint(2, batch, &mut body);
    // 1 : le lot s'est bien passé.
    field_varint(3, 1, &mut body);
    tokio::time::timeout(
        REPLY_TIMEOUT,
        client.call_service_method(COMPLETE_BATCH_METHOD, &body),
    )
    .await
    .map_err(|_| Error::Other("Steam n'a pas répondu".into()))?
    .map_err(|error| Error::Other(format!("clôture du lot refusée ({error})")))?;
    Ok(())
}

/// Un bloc d'envoi : où et comment poser une tranche du fichier.
#[derive(Debug, Default, PartialEq, Eq)]
struct Block {
    host: String,
    path: String,
    https: bool,
    /// Méthode HTTP demandée (1 GET, 2 HEAD, 3 POST, 4 PUT).
    method: u32,
    headers: Vec<(String, String)>,
    offset: usize,
    length: usize,
    body: Option<Vec<u8>>,
}

/// Réponse de `ClientBeginFileUpload#1`.
#[derive(Debug, Default)]
struct Blocks {
    encrypted: bool,
    requests: Vec<Block>,
}

/// Lit les blocs demandés par Steam : chiffrement (1), puis les blocs (2).
fn parse_blocks(body: &[u8]) -> Blocks {
    let mut blocks = Blocks::default();
    let mut reader = Reader::new(body);
    while let Some((number, field)) = reader.next() {
        match (number, field) {
            (1, Field::Varint(value)) => blocks.encrypted = value != 0,
            (2, Field::Bytes(entry)) => blocks.requests.push(parse_block(entry)),
            _ => {}
        }
    }
    blocks
}

/// Une entrée `ClientCloudFileUploadBlockDetails`.
fn parse_block(entry: &[u8]) -> Block {
    let mut block = Block::default();
    let mut reader = Reader::new(entry);
    while let Some((number, field)) = reader.next() {
        match (number, field) {
            (1, Field::Bytes(text)) => block.host = String::from_utf8_lossy(text).into_owned(),
            (2, Field::Bytes(text)) => block.path = String::from_utf8_lossy(text).into_owned(),
            (3, Field::Varint(value)) => block.https = value != 0,
            (4, Field::Varint(value)) => block.method = u32::try_from(value).unwrap_or(0),
            (5, Field::Bytes(header)) => {
                if let Some((name, value)) = parse_header(header) {
                    block.headers.push((name, value));
                }
            }
            (6, Field::Varint(value)) => block.offset = value as usize,
            (7, Field::Varint(value)) => block.length = value as usize,
            (8, Field::Bytes(data)) => block.body = Some(data.to_vec()),
            _ => {}
        }
    }
    block
}

/// Un en-tête HTTP de la requête d'envoi : nom (1) et valeur (2).
fn parse_header(entry: &[u8]) -> Option<(String, String)> {
    let mut name = String::new();
    let mut value = String::new();
    let mut reader = Reader::new(entry);
    while let Some((number, field)) = reader.next() {
        match (number, field) {
            (1, Field::Bytes(text)) => name = String::from_utf8_lossy(text).into_owned(),
            (2, Field::Bytes(text)) => value = String::from_utf8_lossy(text).into_owned(),
            _ => {}
        }
    }
    (!name.is_empty()).then_some((name, value))
}

/// Pose une tranche du fichier à l'adresse donnée.
async fn send_block(block: &Block, data: &[u8]) -> Result<(), Error> {
    let payload = match &block.body {
        Some(body) => body.clone(),
        None => {
            let end = block.offset.saturating_add(block.length).min(data.len());
            if block.offset > data.len() {
                return Err(Error::Other(format!(
                    "bloc hors du fichier ({} sur {})",
                    block.offset,
                    data.len()
                )));
            }
            data[block.offset..end].to_vec()
        }
    };
    let scheme = if block.https { "https" } else { "http" };
    let url = format!("{scheme}://{}{}", block.host, block.path);
    let http = reqwest::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .map_err(|error| Error::Http(error.to_string()))?;
    let request = match block.method {
        1 => http.get(&url),
        2 => http.head(&url),
        3 => http.post(&url),
        // L'envoi d'un bloc est un PUT ; une réponse sans méthode n'en donne
        // pas d'autre qui ait un sens.
        _ => http.put(&url),
    };
    let mut request = request.body(payload);
    for (name, value) in &block.headers {
        // Ces deux en-têtes sont recalculés par le client HTTP : les imposer
        // ferait échouer la requête.
        let lower = name.to_ascii_lowercase();
        if lower == "host" || lower == "content-length" {
            continue;
        }
        request = request.header(name.as_str(), value.as_str());
    }
    let response = request
        .send()
        .await
        .map_err(|error| Error::Http(format!("envoi d'un bloc : {error}")))?;
    if !response.status().is_success() {
        return Err(Error::Http(format!(
            "envoi d'un bloc : {}",
            response.status()
        )));
    }
    Ok(())
}

/// Dossier local d'une sauvegarde cloud, à partir du préfixe déclaré par le
/// jeu (`%WinAppDataLocal%MotorSlice/Saved/SaveGames/`, `win64_save/`…).
///
/// Steam nomme ses racines par des jetons Windows ; sous Proton elles tombent
/// dans `drive_c`. Un jeton inconnu rend `None` : mieux vaut ne rien écrire
/// qu'un fichier au mauvais endroit.
pub fn local_dir(prefix: &str, install_dir: &std::path::Path, prefix_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let prefix = prefix.trim();
    let (token, rest) = match prefix.strip_prefix('%') {
        Some(rest) => {
            let (token, after) = rest.split_once('%')?;
            (token, after.trim_start_matches(['/', '\\']))
        }
        // Chemin sans jeton : il part de l'installation du jeu.
        None => ("GameInstall", prefix.trim_start_matches(['/', '\\'])),
    };
    let relative = rest.replace('\\', "/");
    let relative = relative.trim_end_matches('/');
    // Un jeton ne doit pas servir de tremplin hors du dossier visé :
    // `%GameInstall%../ailleurs` reste refusé.
    if relative.split('/').any(|part| part == "..") {
        return None;
    }
    // Un chemin qui part de l'installation du jeu n'a pas besoin du préfixe
    // Proton : Melatonin écrit `save.json` à côté de son exécutable, et le
    // préfixe n'existe pas encore à la première reprise. L'exiger laissait la
    // sauvegarde cloud dans le cloud.
    if token.eq_ignore_ascii_case("gameinstall") {
        return Some(install_dir.join(relative));
    }
    let windows = drive_c(prefix_dir)?;
    let base = match token.to_ascii_lowercase().as_str() {
        // Dossier de l'utilisateur Windows : certains jeux le nomment
        // directement (`%USERPROFILE%/AppData/...`) au lieu d'une racine
        // dédiée.
        "userprofile" => windows.join("users/steamuser"),
        "winmydocuments" => windows.join("users/steamuser/Documents"),
        "winsavedgames" => windows.join("users/steamuser/Saved Games"),
        "winappdatalocal" => windows.join("users/steamuser/AppData/Local"),
        "winappdatalocallow" => windows.join("users/steamuser/AppData/LocalLow"),
        "winappdataroaming" => windows.join("users/steamuser/AppData/Roaming"),
        "winpublic" => windows.join("users/Public"),
        "winprogramdata" => windows.join("ProgramData"),
        "windir" => windows.join("windows"),
        _ => return None,
    };
    Some(base.join(relative))
}

/// Chemin local d'un fichier cloud. Le nom peut porter lui-même son jeton de
/// racine (`%GameInstall%save.json`) : Melatonin ne déclare aucun dossier et
/// Steam range ainsi ses deux sauvegardes. Dans ce cas c'est le nom qui donne
/// la racine, et le préfixe du fichier est vide.
///
/// Un nom qui remonte (`..`) est refusé : Steam ne doit pas écrire hors du
/// dossier du jeu.
pub fn local_file(
    prefix: &str,
    name: &str,
    install_dir: &std::path::Path,
    prefix_dir: &std::path::Path,
) -> Option<std::path::PathBuf> {
    let name = name.trim().replace('\\', "/");
    if name
        .split('/')
        .any(|part| part.is_empty() || part == "." || part.ends_with(".."))
    {
        return None;
    }
    if name.starts_with('%') {
        return local_dir(&name, install_dir, prefix_dir);
    }
    let mut path = local_dir(prefix, install_dir, prefix_dir)?;
    for part in name.split('/') {
        path.push(part);
    }
    Some(path)
}

/// `drive_c` du préfixe : Proton range le sien sous `pfx`, un préfixe créé par
/// Wine peut n'avoir que `drive_c`.
pub fn drive_c(prefix_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    [prefix_dir.join("pfx/drive_c"), prefix_dir.join("drive_c")]
        .into_iter()
        .find(|path| path.is_dir())
}

/// Réponse de `ClientGetAppQuotaUsage#1`.
fn parse_quota(body: &[u8], state: &mut CloudSaves) {
    let mut reader = Reader::new(body);
    while let Some((number, field)) = reader.next() {
        let Field::Varint(value) = field else {
            continue;
        };
        match number {
            1 => state.used_files = u32::try_from(value).ok(),
            2 => state.used_bytes = Some(value),
            3 => state.max_files = u32::try_from(value).ok().filter(|max| *max > 0),
            4 => state.max_bytes = Some(value).filter(|max| *max > 0),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cm::proto::field_bytes;

    /// Corps d'une réponse de liste : un changement (1), deux fichiers (2) dont
    /// un supprimé, et un dossier déclaré (4).
    fn changelist() -> Vec<u8> {
        let mut body = Vec::new();
        field_varint(1, 42, &mut body);
        let mut file = Vec::new();
        field_bytes(1, b"saves/slot1.sav", &mut file);
        field_varint(3, 1_700_000_000, &mut file);
        field_varint(4, 2048, &mut file);
        field_bytes(2, &file, &mut body);
        // Fichier retiré du cloud : nom vide dans l'historique.
        let mut removed = Vec::new();
        field_varint(3, 1_600_000_000, &mut removed);
        field_bytes(2, &removed, &mut body);
        field_bytes(4, b"%USERPROFILE%/Documents/My Games/Jeu", &mut body);
        body
    }

    #[test]
    fn the_cloud_list_names_files_their_size_and_date() {
        let state = parse_changelist(&changelist());
        assert_eq!(state.change_number, 42);
        assert_eq!(state.files.len(), 1, "le fichier supprimé ne compte pas");
        assert_eq!(state.files[0].name, "saves/slot1.sav");
        assert_eq!(state.files[0].size, 2048);
        assert_eq!(state.files[0].modified, 1_700_000_000);
        assert_eq!(state.total_bytes(), 2048);
        assert_eq!(state.last_modified(), Some(1_700_000_000));
        assert_eq!(
            state.prefixes,
            vec!["%USERPROFILE%/Documents/My Games/Jeu".to_string()]
        );
        assert!(state.has_saves());
    }

    #[test]
    fn a_game_without_cloud_has_nothing_to_show() {
        let state = parse_changelist(&[]);
        assert!(!state.has_saves());
        assert_eq!(state.total_bytes(), 0);
        assert_eq!(state.last_modified(), None);
    }

    /// Préfixe Proton de test : `drive_c` avec le dossier utilisateur.
    fn proton_prefix(dir: &std::path::Path) -> std::path::PathBuf {
        let users = dir.join("pfx/drive_c/users/steamuser/AppData/Local");
        std::fs::create_dir_all(&users).unwrap();
        dir.join("pfx")
    }

    #[test]
    fn a_declared_root_lands_in_the_proton_prefix() {
        let temp = tempfile::tempdir().unwrap();
        proton_prefix(temp.path());
        let install = temp.path().join("install");
        let dir = local_dir(
            "%WinAppDataLocal%MotorSlice/Saved/SaveGames/",
            &install,
            &temp.path().join("pfx"),
        )
        .unwrap();
        assert_eq!(
            dir,
            temp.path().join(
                "pfx/drive_c/users/steamuser/AppData/Local/MotorSlice/Saved/SaveGames"
            )
        );
    }

    #[test]
    fn a_relative_root_lands_in_the_game_folder() {
        let temp = tempfile::tempdir().unwrap();
        proton_prefix(temp.path());
        let install = temp.path().join("install");
        // Requiem range ses sauvegardes à côté de son exécutable.
        let dir = local_dir("win64_save/", &install, &temp.path().join("pfx")).unwrap();
        assert_eq!(dir, install.join("win64_save"));
        // Celui qui nomme le jeu part de l'installation aussi.
        let dir = local_dir(
            "%GameInstall%portal2/SAVE/7656/",
            &install,
            &temp.path().join("pfx"),
        )
        .unwrap();
        assert_eq!(dir, install.join("portal2/SAVE/7656"));
    }

    #[test]
    fn a_save_next_to_the_game_needs_no_prefix() {
        // Melatonin publie `save.json` sans dossier déclaré : la reprise visait
        // le préfixe Proton, absent avant le premier lancement, et échouait.
        let temp = tempfile::tempdir().unwrap();
        let install = temp.path().join("install");
        let absent = temp.path().join("pfx-absent");
        assert_eq!(local_dir("", &install, &absent).unwrap(), install);
        assert_eq!(
            local_dir("%GameInstall%/saves/", &install, &absent).unwrap(),
            install.join("saves")
        );
    }

    #[test]
    fn the_windows_user_folder_is_known() {
        let temp = tempfile::tempdir().unwrap();
        proton_prefix(temp.path());
        let dir = local_dir(
            "%USERPROFILE%/AppData/LocalLow/Studio/Jeu",
            temp.path(),
            &temp.path().join("pfx"),
        )
        .unwrap();
        assert_eq!(
            dir,
            temp.path()
                .join("pfx/drive_c/users/steamuser/AppData/LocalLow/Studio/Jeu")
        );
    }

    #[test]
    fn the_local_low_root_is_known() {
        // Pizza Possum publie `%WinAppDataLocalLow%Cosy Computer/PizzaPossum/storage/`.
        let temp = tempfile::tempdir().unwrap();
        proton_prefix(temp.path());
        let install = temp.path().join("install");
        let path = local_file(
            "%WinAppDataLocalLow%Cosy Computer/PizzaPossum/storage/",
            "save",
            &install,
            &temp.path().join("pfx"),
        )
        .unwrap();
        assert_eq!(
            path,
            temp.path().join(
                "pfx/drive_c/users/steamuser/AppData/LocalLow/Cosy Computer/PizzaPossum/storage/save"
            )
        );
    }

    #[test]
    fn a_name_carrying_its_root_lands_next_to_the_game() {
        // Le cas Melatonin : préfixe vide, nom `%GameInstall%save.json`.
        let temp = tempfile::tempdir().unwrap();
        let install = temp.path().join("install");
        let absent = temp.path().join("pfx-absent");
        assert_eq!(
            local_file("", "%GameInstall%save.json", &install, &absent).unwrap(),
            install.join("save.json")
        );
        assert_eq!(
            local_file("", "%GameInstall%backup.json", &install, &absent).unwrap(),
            install.join("backup.json")
        );
        // Une remontée de dossier reste refusée, même dans le nom.
        assert_eq!(
            local_file("", "%GameInstall%../ailleurs", &install, &absent),
            None
        );
        assert_eq!(local_file("", "", &install, &absent), None);
    }

    #[test]
    fn an_unknown_root_writes_nothing() {
        let temp = tempfile::tempdir().unwrap();
        proton_prefix(temp.path());
        assert_eq!(
            local_dir("%MacBogus%saves/", temp.path(), &temp.path().join("pfx")),
            None
        );
    }

    #[test]
    fn the_download_names_the_cdn_and_an_encrypted_file() {
        let mut body = Vec::new();
        field_varint(2, 4096, &mut body);
        field_bytes(7, b"cloudflare.steamstatic.com", &mut body);
        field_bytes(8, b"/cloud/1/abc", &mut body);
        field_varint(9, 1, &mut body);
        let download = parse_download(&body).unwrap();
        assert_eq!(download.size, 4096);
        assert_eq!(download.host, "cloudflare.steamstatic.com");
        assert_eq!(download.path, "/cloud/1/abc");
        assert!(download.https);
        assert!(!download.encrypted);

        let mut encrypted = body.clone();
        field_varint(11, 1, &mut encrypted);
        assert!(parse_download(&encrypted).unwrap().encrypted);
        // Sans adresse, rien à télécharger.
        assert!(parse_download(&[]).is_none());
    }

    #[test]
    fn a_file_knows_which_declared_root_it_belongs_to() {
        let state = parse_changelist(&changelist());
        assert_eq!(
            state.files[0].prefix,
            "%USERPROFILE%/Documents/My Games/Jeu".to_string()
        );
    }

    #[test]
    fn the_quota_says_what_the_account_uses() {
        let mut body = Vec::new();
        field_varint(1, 3, &mut body);
        field_varint(2, 12_345, &mut body);
        field_varint(3, 128, &mut body);
        field_varint(4, 1_073_741_824, &mut body);
        let mut state = CloudSaves::default();
        parse_quota(&body, &mut state);
        assert_eq!(state.used_files, Some(3));
        assert_eq!(state.used_bytes, Some(12_345));
        assert_eq!(state.max_files, Some(128));
        assert_eq!(state.max_bytes, Some(1_073_741_824));
    }
}
