//! Manifeste de build Epic : lecture du format binaire et chemins des chunks.
//!
//! Le launcher Epic ne publie pas d'archive : il publie un **manifeste
//! binaire** qui liste les fichiers du jeu, découpés en *chunks* adressés par
//! empreinte. Chaque fichier est la concaténation de morceaux de chunks ; il
//! suffit donc de télécharger chaque chunk une fois et de recopier les tranches
//! à leur place.
//!
//! Le format suit celui que lit Legendary (`legendary/models/manifest.py`) :
//! en-tête de 41 octets (73 depuis la version 22, avec le GUID de secret et le
//! tag de chiffrement), puis des sections `taille / version / données`. Les
//! entiers sont little-endian, les chaînes sont des `FString` (longueur signée
//! en tête).

use crate::common::{Error, Result};

/// Nombre magique d'un manifeste de build.
const MAGIC: u32 = 0x44BE_C00C;

/// Un chunk publié : son empreinte, son groupe et sa taille compressée.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub guid: [u32; 4],
    /// Empreinte 64 bits utilisée dans le nom du fichier.
    pub hash: u64,
    /// Groupe (sous-dossier `00` à `99`) du chunk sur le CDN.
    pub group: u8,
    /// Taille du chunk une fois décompressé.
    pub window: u32,
    /// Taille du fichier téléchargé, négative pour un chunk sans données.
    pub size: i64,
    /// GUID de secret des manifestes 22 et suivants.
    pub secret: Option<[u32; 4]>,
}

impl Chunk {
    /// Poids à télécharger, nul pour un chunk sans données.
    pub fn download_size(&self) -> u64 {
        if self.size > 0 {
            self.size as u64
        } else {
            0
        }
    }
}

/// Une tranche de chunk recopiée dans un fichier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Part {
    pub chunk: [u32; 4],
    pub offset: u32,
    pub size: u32,
}

/// Un fichier du jeu, découpé en tranches de chunks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub path: String,
    pub sha1: [u8; 20],
    /// Bit 2 : le fichier est exécutable.
    pub flags: u8,
    pub parts: Vec<Part>,
}

impl FileEntry {
    /// Taille du fichier, somme de ses tranches.
    pub fn size(&self) -> u64 {
        self.parts.iter().map(|part| part.size as u64).sum()
    }

    pub fn executable(&self) -> bool {
        self.flags & 0b100 != 0
    }
}

/// Ce que le manifeste publie : les chunks puis les fichiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub app_name: String,
    pub build_version: String,
    /// Niveau de fonctionnalité, qui décide du nom des chunks sur le CDN.
    pub feature_level: u32,
    pub chunks: Vec<Chunk>,
    pub files: Vec<FileEntry>,
}

impl Manifest {
    /// Poids total à télécharger : la somme des chunks.
    pub fn download_size(&self) -> u64 {
        self.chunks.iter().map(Chunk::download_size).sum()
    }

    /// Poids installé : la somme des fichiers.
    pub fn install_size(&self) -> u64 {
        self.files.iter().map(FileEntry::size).sum()
    }

    /// Dossier des chunks pour ce niveau de fonctionnalité.
    pub fn chunk_dir(&self) -> &'static str {
        chunk_dir(self.feature_level)
    }

    /// Chemin du chunk sous le dossier CDN (`ChunksV4/07/HASH_GUID.chunk`).
    pub fn chunk_path(&self, chunk: &Chunk) -> String {
        chunk_path(self.feature_level, chunk)
    }

    /// Chunk par GUID, pour recoller les tranches.
    pub fn chunk(&self, guid: &[u32; 4]) -> Option<&Chunk> {
        self.chunks.iter().find(|chunk| &chunk.guid == guid)
    }
}

/// Dossier des chunks selon le niveau de fonctionnalité du manifeste.
pub fn chunk_dir(feature_level: u32) -> &'static str {
    match feature_level {
        0..=2 => "Chunks",
        3..=5 => "ChunksV2",
        6..=14 => "ChunksV3",
        15..=21 => "ChunksV4",
        _ => "ChunksV5",
    }
}

/// Chemin d'un chunk sous le dossier CDN.
///
/// Avant la version 22, le nom est `GROUPE/HASH_GUID.chunk` en hexadécimal.
/// Depuis, il est encodé en base64 URL, rangé sous son secret (ou `plain`) :
/// le CDN refuse les noms trop longs en hexadécimal.
pub fn chunk_path(feature_level: u32, chunk: &Chunk) -> String {
    if feature_level >= 22 {
        let secret = chunk
            .secret
            .filter(|secret| secret.iter().any(|word| *word != 0))
            .map(|guid| guid_base64(&guid))
            .unwrap_or_else(|| "plain".to_string());
        let mut hash = Vec::with_capacity(8);
        hash.extend_from_slice(&chunk.hash.to_le_bytes());
        format!(
            "{secret}/{:02}/{}_{}.chunk",
            chunk.group,
            base64_url(&hash),
            guid_base64(&chunk.guid)
        )
    } else {
        let guid = chunk
            .guid
            .iter()
            .map(|word| format!("{word:08X}"))
            .collect::<String>();
        format!("{:02}/{:016X}_{guid}.chunk", chunk.group, chunk.hash)
    }
}

/// GUID encodé comme l'attend le CDN : quatre mots little-endian en base64
/// URL, sans remplissage.
fn guid_base64(guid: &[u32; 4]) -> String {
    let mut bytes = Vec::with_capacity(16);
    for word in guid {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    base64_url(&bytes)
}

/// Base64 URL sans remplissage (RFC 4648 §5).
fn base64_url(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for block in input.chunks(3) {
        let bytes = [
            block[0],
            *block.get(1).unwrap_or(&0),
            *block.get(2).unwrap_or(&0),
        ];
        let value = ((bytes[0] as u32) << 16) | ((bytes[1] as u32) << 8) | bytes[2] as u32;
        out.push(ALPHABET[(value >> 18) as usize & 63] as char);
        out.push(ALPHABET[(value >> 12) as usize & 63] as char);
        if block.len() > 1 {
            out.push(ALPHABET[(value >> 6) as usize & 63] as char);
        }
        if block.len() > 2 {
            out.push(ALPHABET[value as usize & 63] as char);
        }
    }
    out
}

/// Lecture d'un tampon, décalage après décalage.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .at
            .checked_add(count)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| Error::Parse("manifeste Epic tronqué".into()))?;
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn u64(&mut self) -> Result<u64> {
        let bytes = self.take(8)?;
        let mut value = [0u8; 8];
        value.copy_from_slice(bytes);
        Ok(u64::from_le_bytes(value))
    }

    fn i64(&mut self) -> Result<i64> {
        Ok(self.u64()? as i64)
    }

    fn guid(&mut self) -> Result<[u32; 4]> {
        Ok([self.u32()?, self.u32()?, self.u32()?, self.u32()?])
    }

    /// `FString` : longueur signée — positive en octets, négative en unités
    /// UTF-16 — NUL final compris.
    fn string(&mut self) -> Result<String> {
        let length = self.u32()? as i32;
        if length == 0 {
            return Ok(String::new());
        }
        if length > 0 {
            let bytes = self.take(length as usize)?;
            let text = &bytes[..bytes.len().saturating_sub(1)];
            Ok(String::from_utf8_lossy(text).into_owned())
        } else {
            let units = length.unsigned_abs() as usize;
            let bytes = self.take(units * 2)?;
            let words: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .take(units.saturating_sub(1))
                .collect();
            Ok(String::from_utf16_lossy(&words))
        }
    }
}

/// Lit un manifeste de build. Les champs inutiles à l'installation (champs
/// personnalisés, empreintes MD5, prérequis) sont sautés.
pub fn parse(bytes: &[u8]) -> Result<Manifest> {
    // Les builds récents publient un manifeste **JSON** là où les anciens
    // servaient un binaire : sans cette bifurcation, l'installation refusait
    // tous les jeux encore mis à jour.
    if bytes.iter().find(|byte| !byte.is_ascii_whitespace()) == Some(&b'{') {
        return parse_json(bytes);
    }
    let mut reader = Reader::new(bytes);
    let magic = reader.u32()?;
    if magic != MAGIC {
        return Err(Error::Parse(format!(
            "ce n'est pas un manifeste Epic (nombre magique {magic:08X})"
        )));
    }
    let header_size = reader.u32()? as usize;
    let body_size = reader.u32()? as usize;
    let stored_size = reader.u32()? as usize;
    let _sha1 = reader.take(20)?;
    let stored_as = reader.u8()?;
    let _version = reader.u32()?;
    // La taille d'en-tête fait foi : les versions récentes y ajoutent le GUID
    // de secret et le tag de chiffrement, d'où ce saut.
    if header_size < reader.at {
        return Err(Error::Parse("en-tête de manifeste incohérent".into()));
    }
    if stored_as & 0b10 != 0 {
        return Err(Error::Unavailable(
            "ce build Epic est chiffré : Lapinux ne sait pas l'ouvrir",
        ));
    }
    let body = if stored_as & 1 != 0 {
        let mut out = Vec::new();
        let mut decoder = flate2::read::ZlibDecoder::new(&bytes[header_size..]);
        std::io::Read::read_to_end(&mut decoder, &mut out)
            .map_err(|error| Error::Parse(format!("manifeste compressé illisible : {error}")))?;
        out
    } else {
        let end = header_size
            .checked_add(stored_size)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| Error::Parse("manifeste Epic tronqué".into()))?;
        bytes[header_size..end].to_vec()
    };
    if body.len() < body_size.min(stored_size).max(1) {
        return Err(Error::Parse("corps de manifeste incomplet".into()));
    }

    let mut reader = Reader::new(&body);
    // Meta
    let _meta_size = reader.u32()?;
    let meta_version = reader.u8()?;
    let feature_level = reader.u32()?;
    let _is_file_data = reader.u8()?;
    let _app_id = reader.u32()?;
    let app_name = reader.string()?;
    let build_version = reader.string()?;
    let _launch_exe = reader.string()?;
    let _launch_command = reader.string()?;
    let prereq_count = reader.u32()?;
    for _ in 0..prereq_count {
        let _ = reader.string()?;
    }
    let _prereq_name = reader.string()?;
    let _prereq_path = reader.string()?;
    let _prereq_args = reader.string()?;
    if meta_version >= 1 {
        let _build_id = reader.string()?;
    }
    if meta_version >= 2 {
        let _uninstall_path = reader.string()?;
        let _uninstall_args = reader.string()?;
    }

    // Chunks : les colonnes sont rangées par champ, pas par chunk.
    let _chunk_section = reader.u32()?;
    let _chunk_version = reader.u8()?;
    let chunk_count = reader.u32()? as usize;
    let mut chunks = Vec::with_capacity(chunk_count.min(4096));
    let mut guids = Vec::with_capacity(chunk_count.min(4096));
    for _ in 0..chunk_count {
        guids.push(reader.guid()?);
    }
    let mut hashes = Vec::with_capacity(chunk_count);
    for _ in 0..chunk_count {
        hashes.push(reader.u64()?);
    }
    for _ in 0..chunk_count {
        let _sha1 = reader.take(20)?;
    }
    let mut groups = Vec::with_capacity(chunk_count);
    for _ in 0..chunk_count {
        groups.push(reader.u8()?);
    }
    let mut windows = Vec::with_capacity(chunk_count);
    for _ in 0..chunk_count {
        windows.push(reader.u32()?);
    }
    let mut sizes = Vec::with_capacity(chunk_count);
    for _ in 0..chunk_count {
        sizes.push(reader.i64()?);
    }
    let mut secrets = Vec::with_capacity(chunk_count);
    if feature_level >= 22 {
        for _ in 0..chunk_count {
            secrets.push(reader.guid()?);
        }
        for _ in 0..chunk_count {
            let _window_compressed = reader.u32()?;
        }
        for _ in 0..chunk_count {
            let _tag = reader.take(16)?;
        }
    }
    for (index, guid) in guids.iter().enumerate() {
        chunks.push(Chunk {
            guid: *guid,
            hash: hashes[index],
            group: groups[index],
            window: windows[index],
            size: sizes[index],
            secret: secrets.get(index).copied(),
        });
    }

    // Fichiers : mêmes colonnes, puis les tranches fichier par fichier.
    let _file_section = reader.u32()?;
    let file_version = reader.u8()?;
    let file_count = reader.u32()? as usize;
    let mut paths = Vec::with_capacity(file_count.min(4096));
    for _ in 0..file_count {
        paths.push(reader.string()?);
    }
    let mut symlinks = Vec::with_capacity(file_count.min(4096));
    for _ in 0..file_count {
        symlinks.push(reader.string()?);
    }
    for _ in 0..file_count {
        let _sha1 = reader.take(20)?;
    }
    let mut flags = Vec::with_capacity(file_count);
    for _ in 0..file_count {
        flags.push(reader.u8()?);
    }
    // Les étiquettes d'installation sont rangées pour tous les fichiers avant
    // les tranches : les lire fichier par fichier décalait tout le reste, et
    // l'installation échouait sur « manifeste tronqué ».
    for _ in 0..file_count {
        let tags = reader.u32()? as usize;
        for _ in 0..tags {
            let _tag = reader.string()?;
        }
    }
    let mut files = Vec::with_capacity(file_count);
    for index in 0..file_count {
        let part_count = reader.u32()? as usize;
        let mut parts = Vec::with_capacity(part_count.min(4096));
        for _ in 0..part_count {
            let _record_size = reader.u32()?;
            let chunk = reader.guid()?;
            let offset = reader.u32()?;
            let size = reader.u32()?;
            parts.push(Part {
                chunk,
                offset,
                size,
            });
        }
        // Un lien symbolique n'a ni données ni tranches : il est recréé tel
        // quel à l'installation.
        let path = if symlinks[index].is_empty() {
            paths[index].clone()
        } else {
            symlinks[index].clone()
        };
        files.push(FileEntry {
            path,
            sha1: [0u8; 20],
            flags: flags[index],
            parts,
        });
    }
    // Les empreintes MD5, types MIME et SHA-256 qui suivent (version 1 et 2 de
    // la liste) ne servent pas à l'installation : on s'arrête là.

    let _ = file_version;
    Ok(Manifest {
        app_name,
        build_version,
        feature_level,
        chunks,
        files,
    })
}

/// Nombre « blob » d'un manifeste JSON Epic : trois chiffres décimaux par
/// octet, assemblés en little-endian (« 013000000000 » = 13).
fn blob_bytes(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if text.is_empty() || text.len() % 3 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 3);
    for group in text.as_bytes().chunks(3) {
        let value: u8 = std::str::from_utf8(group).ok()?.parse().ok()?;
        out.push(value);
    }
    Some(out)
}

/// Blob décodé en entier, little-endian, sur la largeur utiles des octets lus.
fn blob_int(text: &str) -> Option<i64> {
    let bytes = blob_bytes(text)?;
    if bytes.len() > 8 {
        return None;
    }
    let mut buffer = [0u8; 8];
    buffer[..bytes.len()].copy_from_slice(&bytes);
    Some(i64::from_le_bytes(buffer))
}

fn blob_u64(text: &str) -> Option<u64> {
    blob_int(text).map(|value| value as u64)
}

fn blob_u32(text: &str) -> Option<u32> {
    let bytes = blob_bytes(text)?;
    if bytes.len() > 4 {
        return None;
    }
    let mut buffer = [0u8; 4];
    buffer[..bytes.len()].copy_from_slice(&bytes);
    Some(u32::from_le_bytes(buffer))
}

/// GUID écrit en 32 chiffres hexadécimaux, quatre mots dans l'ordre du fichier.
fn guid_from_hex(text: &str) -> Option<[u32; 4]> {
    let text = text.trim();
    if text.len() != 32 {
        return None;
    }
    let mut words = [0u32; 4];
    for (index, word) in words.iter_mut().enumerate() {
        *word = u32::from_str_radix(&text[index * 4..index * 4 + 4], 16).ok()?;
    }
    Some(words)
}

/// Manifeste JSON : les builds récents le préfèrent au binaire. Les mêmes
/// chunks et fichiers s'y lisent, en nombres « blob » et en GUID textuels.
fn parse_json(bytes: &[u8]) -> Result<Manifest> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| Error::Parse(format!("manifeste Epic JSON illisible : {error}")))?;
    let text = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    // Le niveau de fonctionnalité tient dans `ManifestFileVersion` ; c'est lui
    // qui choisit le dossier des chunks (V3 pour les manifestes JSON).
    let feature_level = blob_u32(&text("ManifestFileVersion")).unwrap_or(13);
    let hashes = value
        .get("ChunkHashList")
        .and_then(serde_json::Value::as_object);
    let sizes = value
        .get("ChunkFilesizeList")
        .and_then(serde_json::Value::as_object);
    let groups = value
        .get("DataGroupList")
        .and_then(serde_json::Value::as_object);
    let mut chunks = Vec::new();
    if let Some(hashes) = hashes {
        for (guid_text, hash_text) in hashes {
            let Some(guid) = guid_from_hex(guid_text) else {
                continue;
            };
            fn lookup<'a>(
                list: Option<&'a serde_json::Map<String, serde_json::Value>>,
                guid: &str,
            ) -> Option<&'a str> {
                list.and_then(|list| list.get(guid))
                    .and_then(serde_json::Value::as_str)
            }
            chunks.push(Chunk {
                guid,
                hash: hash_text.as_str().and_then(blob_u64).unwrap_or(0),
                group: lookup(groups, guid_text)
                    .and_then(blob_bytes)
                    .and_then(|bytes| bytes.first().copied())
                    .unwrap_or(0),
                // Le JSON ne publie pas la taille décompressée : elle ne sert
                // qu'aux manifestes chiffrés, que Lapinux refuse déjà.
                window: 0,
                size: lookup(sizes, guid_text).and_then(blob_int).unwrap_or(0),
                secret: None,
            });
        }
    }
    let mut files = Vec::new();
    if let Some(list) = value
        .get("FileManifestList")
        .and_then(serde_json::Value::as_array)
    {
        for file in list {
            let path = file
                .get("Filename")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            if path.is_empty() {
                continue;
            }
            let mut flags = 0u8;
            if file
                .get("bIsUnixExecutable")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
            {
                flags |= 0b100;
            }
            let mut parts = Vec::new();
            if let Some(list) = file
                .get("FileChunkParts")
                .and_then(serde_json::Value::as_array)
            {
                for part in list {
                    let Some(guid) = part
                        .get("Guid")
                        .and_then(serde_json::Value::as_str)
                        .and_then(guid_from_hex)
                    else {
                        continue;
                    };
                    parts.push(Part {
                        chunk: guid,
                        offset: part
                            .get("Offset")
                            .and_then(serde_json::Value::as_str)
                            .and_then(blob_u32)
                            .unwrap_or(0),
                        size: part
                            .get("Size")
                            .and_then(serde_json::Value::as_str)
                            .and_then(blob_u32)
                            .unwrap_or(0),
                    });
                }
            }
            files.push(FileEntry {
                path,
                sha1: [0u8; 20],
                flags,
                parts,
            });
        }
    }
    Ok(Manifest {
        app_name: text("AppNameString"),
        build_version: text("BuildVersionString"),
        feature_level,
        chunks,
        files,
    })
}

/// Nombre magique de l'en-tête d'un chunk publié par le CDN.
const CHUNK_MAGIC: u32 = 0xB1FE_3AA2;

/// Données utiles d'un chunk téléchargé.
///
/// Le CDN préfixe chaque chunk d'un en-tête (magie, GUID, empreinte, drapeaux,
/// SHA-1) et compresse souvent le corps en zlib. Recopier le fichier tel quel
/// plaçait des octets d'en-tête dans le jeu : chaque fichier était corrompu.
pub fn chunk_payload(bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.len() < 12 {
        return Err(Error::Parse("chunk Epic vide".into()));
    }
    let magic = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    if magic != CHUNK_MAGIC {
        // Chunk déjà nu (anciens builds) : rien à retirer.
        return Ok(bytes.to_vec());
    }
    let header_size = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    // L'en-tête vaut 62 octets (version 2) ou 66 (version 3) ; ses champs fixes
    // placent « storedAs » au 41e octet dans les deux cas.
    if header_size < 12 || header_size > bytes.len() {
        return Err(Error::Parse("en-tête de chunk Epic incohérent".into()));
    }
    let stored_as = bytes.get(40).copied().unwrap_or(0);
    let payload = &bytes[header_size..];
    if stored_as & 1 == 0 {
        return Ok(payload.to_vec());
    }
    let mut out = Vec::new();
    let mut decoder = flate2::read::ZlibDecoder::new(payload);
    std::io::Read::read_to_end(&mut decoder, &mut out)
        .map_err(|error| Error::Parse(format!("chunk Epic compressé illisible : {error}")))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn string(out: &mut Vec<u8>, text: &str) {
        out.extend_from_slice(&((text.len() + 1) as i32).to_le_bytes());
        out.extend_from_slice(text.as_bytes());
        out.push(0);
    }

    fn utf16_string(out: &mut Vec<u8>, text: &str) {
        let units: Vec<u16> = text.encode_utf16().collect();
        out.extend_from_slice(&(-((units.len() + 1) as i32)).to_le_bytes());
        for unit in units {
            out.extend_from_slice(&unit.to_le_bytes());
        }
        out.extend_from_slice(&0u16.to_le_bytes());
    }

    /// Un manifeste minimal, écrit champ par champ dans l'ordre que lit le
    /// parseur — c'est la référence du format.
    fn sample(feature_level: u32) -> Vec<u8> {
        let mut body = Vec::new();
        // Meta
        let meta_at = body.len();
        body.extend_from_slice(&0u32.to_le_bytes());
        body.push(2); // version des méta
        body.extend_from_slice(&feature_level.to_le_bytes());
        body.push(0); // données de fichiers
        body.extend_from_slice(&7u32.to_le_bytes());
        string(&mut body, "Crab");
        string(&mut body, "1.0.0");
        string(&mut body, "Game.exe");
        string(&mut body, "");
        body.extend_from_slice(&0u32.to_le_bytes()); // prérequis
        string(&mut body, "");
        string(&mut body, "");
        string(&mut body, "");
        string(&mut body, "build-id");
        string(&mut body, "");
        string(&mut body, "");
        let meta_size = (body.len() - meta_at) as u32;
        body[meta_at..meta_at + 4].copy_from_slice(&meta_size.to_le_bytes());

        // Chunks
        let chunk_at = body.len();
        body.extend_from_slice(&0u32.to_le_bytes());
        body.push(0);
        body.extend_from_slice(&2u32.to_le_bytes());
        for guid in [[1u32, 2, 3, 4], [9u32, 8, 7, 6]] {
            for word in guid {
                body.extend_from_slice(&word.to_le_bytes());
            }
        }
        body.extend_from_slice(&0x1122_3344_5566_7788u64.to_le_bytes());
        body.extend_from_slice(&0x0102_0304_0506_0708u64.to_le_bytes());
        for _ in 0..2 {
            body.extend_from_slice(&[0u8; 20]);
        }
        body.push(3);
        body.push(7);
        body.extend_from_slice(&64u32.to_le_bytes());
        body.extend_from_slice(&128u32.to_le_bytes());
        body.extend_from_slice(&64i64.to_le_bytes());
        body.extend_from_slice(&128i64.to_le_bytes());
        if feature_level >= 22 {
            for _ in 0..2 {
                body.extend_from_slice(&[0u8; 16]);
            }
            for _ in 0..2 {
                body.extend_from_slice(&32u32.to_le_bytes());
            }
            for _ in 0..2 {
                body.extend_from_slice(&[0u8; 16]);
            }
        }
        let chunk_size = (body.len() - chunk_at) as u32;
        body[chunk_at..chunk_at + 4].copy_from_slice(&chunk_size.to_le_bytes());

        // Fichiers
        let file_at = body.len();
        body.extend_from_slice(&0u32.to_le_bytes());
        body.push(0);
        body.extend_from_slice(&2u32.to_le_bytes());
        string(&mut body, "Game.exe");
        string(&mut body, "data/level.pak");
        string(&mut body, "");
        string(&mut body, "");
        for _ in 0..2 {
            body.extend_from_slice(&[0u8; 20]);
        }
        body.push(0b100); // exécutable
        body.push(0);
        // Le format range les étiquettes d'installation de tous les fichiers
        // avant les tranches : les écrire fichier par fichier décalait tout ce
        // qui suit et faisait échouer la lecture sur « manifeste tronqué ».
        for _ in 0..2 {
            body.extend_from_slice(&0u32.to_le_bytes()); // étiquettes
        }
        for guid in [[1u32, 2, 3, 4], [9u32, 8, 7, 6]] {
            body.extend_from_slice(&1u32.to_le_bytes()); // une tranche
            body.extend_from_slice(&28u32.to_le_bytes());
            for word in guid {
                body.extend_from_slice(&word.to_le_bytes());
            }
            body.extend_from_slice(&0u32.to_le_bytes());
            body.extend_from_slice(&(64u32).to_le_bytes());
        }
        let file_size = (body.len() - file_at) as u32;
        body[file_at..file_at + 4].copy_from_slice(&file_size.to_le_bytes());

        // En-tête
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC.to_le_bytes());
        let header_size = if feature_level >= 22 { 73 } else { 41 };
        out.extend_from_slice(&(header_size as u32).to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&[0u8; 20]);
        out.push(0); // non compressé
        out.extend_from_slice(&4u32.to_le_bytes());
        if feature_level >= 22 {
            out.extend_from_slice(&[0u8; 16]);
            out.extend_from_slice(&[0u8; 16]);
        }
        out.extend_from_slice(&body);
        out
    }

    #[test]
    fn a_legacy_manifest_lists_its_chunks_and_files() {
        let manifest = parse(&sample(21)).expect("manifeste");
        assert_eq!(manifest.app_name, "Crab");
        assert_eq!(manifest.build_version, "1.0.0");
        assert_eq!(manifest.feature_level, 21);
        assert_eq!(manifest.chunks.len(), 2);
        assert_eq!(manifest.chunks[0].group, 3);
        assert_eq!(manifest.chunks[0].hash, 0x1122_3344_5566_7788);
        assert_eq!(manifest.download_size(), 192);
        assert_eq!(manifest.files.len(), 2);
        assert!(manifest.files[0].executable());
        assert!(!manifest.files[1].executable());
        assert_eq!(manifest.files[1].path, "data/level.pak");
        assert_eq!(manifest.install_size(), 128);
        assert_eq!(manifest.chunk_dir(), "ChunksV4");
    }

    #[test]
    fn a_modern_manifest_reads_its_extra_columns() {
        let manifest = parse(&sample(23)).expect("manifeste");
        assert_eq!(manifest.feature_level, 23);
        assert_eq!(manifest.chunks.len(), 2);
        assert_eq!(manifest.chunk_dir(), "ChunksV5");
    }

    #[test]
    fn chunk_paths_follow_the_manifest_version() {
        let manifest = parse(&sample(21)).expect("manifeste");
        let legacy = manifest.chunk_path(&manifest.chunks[0]);
        // Groupe, empreinte 64 bits, puis les quatre mots du GUID.
        assert_eq!(
            legacy,
            "03/1122334455667788_00000001000000020000000300000004.chunk"
        );

        let modern = parse(&sample(23)).expect("manifeste");
        let path = modern.chunk_path(&modern.chunks[1]);
        // Sans secret : rangé sous `plain`, empreinte et GUID en base64 URL.
        assert!(path.starts_with("plain/07/"), "{path}");
        assert!(path.ends_with(".chunk"));
        assert!(!path.contains('='));
        assert!(path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/_.-".contains(c)));
    }

    #[test]
    fn a_foreign_file_is_refused_rather_than_read() {
        let error = parse(b"pas un manifeste").unwrap_err();
        assert!(error.to_string().contains("manifeste Epic"), "{error}");
    }

    #[test]
    fn utf16_strings_survive_the_length_prefix() {
        let mut out = Vec::new();
        utf16_string(&mut out, "Jeu été");
        out.extend_from_slice(&[0u8; 4]);
        let mut reader = Reader::new(&out);
        assert_eq!(reader.string().unwrap(), "Jeu été");
        // Quatre octets restants : la longueur nulle d'une chaîne vide.
        assert_eq!(reader.string().unwrap(), "");
    }

    #[test]
    fn base64_matches_the_url_alphabet() {
        assert_eq!(base64_url(b""), "");
        assert_eq!(base64_url(b"f"), "Zg");
        assert_eq!(base64_url(b"fo"), "Zm8");
        assert_eq!(base64_url(b"foo"), "Zm9v");
        assert_eq!(base64_url(&[0xfb, 0xff]), "-_8");
    }
}
