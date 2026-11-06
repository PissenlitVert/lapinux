//! Sauvegardes cloud des boutiques : ce que le compte garde, dans quel ordre
//! les fichiers se nomment, et les transferts.
//!
//! Epic et GOG ne rangent pas leurs sauvegardes de la même façon : Epic publie
//! une liste de fichiers avec leurs liens signés, GOG un tableau de fichiers
//! sous un dossier par jeu, servi par `cloudstorage.gog.com`. Ce module porte
//! les deux formes et les conversions communes — dates ISO 8601, gzip, empreinte
//! MD5 que GOG demande en `Etag`.

use crate::common::{self, Error, Result};
use serde::{Deserialize, Serialize};

/// Un fichier gardé dans le cloud d'une boutique.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteFile {
    /// Chemin relatif, sans le dossier du jeu quand la boutique en ajoute un.
    pub name: String,
    pub size: u64,
    /// Dernière modification, en secondes Unix (0 quand la boutique se tait).
    pub modified: i64,
    /// Empreinte MD5 que GOG publie : elle dit si le fichier a changé.
    pub hash: String,
    /// Lien de lecture signé (Epic), vide ailleurs.
    #[serde(skip)]
    pub read_url: String,
    /// Lien d'écriture signé (Epic), demandé pour un nom précis.
    #[serde(skip)]
    pub write_url: String,
    /// Nom complet chez la boutique, dossier du jeu compris (GOG).
    #[serde(skip)]
    pub full_name: String,
}

impl RemoteFile {
    /// Nom tel que la carte des réglages l'affiche : le chemin relatif au
    /// dossier de sauvegarde, jamais le nom complet de la boutique.
    pub fn display_name(&self) -> &str {
        &self.name
    }
}

/// Date ISO 8601 (`2026-06-12T09:17:00.123Z`, `2026-06-12T09:17:00+02:00`)
/// ramenée en secondes Unix. Aucune dépendance de date n'est ajoutée pour ces
/// deux formats : le calcul est direct.
pub fn parse_iso8601(text: &str) -> Option<i64> {
    let text = text.trim();
    let bytes = text.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let year: i64 = text.get(0..4)?.parse().ok()?;
    let month: i64 = text.get(5..7)?.parse().ok()?;
    let day: i64 = text.get(8..10)?.parse().ok()?;
    let hour: i64 = text.get(11..13)?.parse().ok()?;
    let minute: i64 = text.get(14..16)?.parse().ok()?;
    let second: i64 = text.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Jours depuis l'époque (algorithme de Howard Hinnant, civil_from_days).
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146097 + day_of_era - 719468;
    let mut stamp = days * 86_400 + hour * 3_600 + minute * 60 + second;
    // Décalage horaire explicite : `+02:00` avance l'heure locale.
    if let Some(position) = text.rfind(['+', '-']) {
        if position > 10 {
            let rest = &text[position + 1..];
            let sign = if bytes[position] == b'-' { -1 } else { 1 };
            let (hours, minutes) = match rest.split_once(':') {
                Some((hours, minutes)) => (
                    hours.parse::<i64>().ok()?,
                    minutes.get(0..2).and_then(|m| m.parse::<i64>().ok()).unwrap_or(0),
                ),
                None if rest.len() >= 4 => (
                    rest[0..2].parse::<i64>().ok()?,
                    rest[2..4].parse::<i64>().ok()?,
                ),
                None => return None,
            };
            // Un décalage « +02:00 » veut dire que l'heure écrite est deux
            // heures en avance sur UTC.
            stamp -= sign * (hours * 3_600 + minutes * 60);
        }
    }
    Some(stamp)
}

/// Date ISO 8601 UTC d'une date Unix, pour l'en-tête que GOG attend.
pub fn format_iso8601(stamp: i64) -> String {
    let days = stamp.div_euclid(86_400);
    let seconds = stamp.rem_euclid(86_400);
    // civil_from_days, même algorithme que ci-dessus, à l'envers.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds / 3_600,
        (seconds % 3_600) / 60,
        seconds % 60
    )
}

/// Corps gzippé d'un fichier et son empreinte MD5, telle que GOG l'attend en
/// `Etag` : l'empreinte porte sur le contenu **compressé**.
pub fn gzip_md5(bytes: &[u8]) -> Result<(Vec<u8>, String)> {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;
    // `mtime` à zéro : le même fichier doit donner le même corps, sinon
    // l'empreinte change à chaque envoi et GOG croit à un fichier neuf.
    let mut encoder = GzEncoder::new(Vec::new(), Compression::new(6));
    encoder
        .write_all(bytes)
        .map_err(|error| Error::Other(format!("compression : {error}")))?;
    let compressed = encoder
        .finish()
        .map_err(|error| Error::Other(format!("compression : {error}")))?;
    let digest = md5_hex(&compressed);
    Ok((compressed, digest))
}

/// Empreinte MD5 en hexadécimal minuscule, sans dépendance de hachage.
pub fn md5_hex(bytes: &[u8]) -> String {
    let digest = md5(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// MD5 (RFC 1321) : GOG s'en sert comme étiquette de fichier, un SHA ne
/// conviendrait pas.
fn md5(input: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];
    let mut message = input.to_vec();
    let bit_len = (input.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_le_bytes());

    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;
    for chunk in message.chunks(64) {
        let mut words = [0u32; 16];
        for (index, word) in words.iter_mut().enumerate() {
            let start = index * 4;
            *word = u32::from_le_bytes([
                chunk[start],
                chunk[start + 1],
                chunk[start + 2],
                chunk[start + 3],
            ]);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | (!b & d), i),
                16..=31 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(K[i]).wrapping_add(words[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    for (index, value) in [a0, b0, c0, d0].iter().enumerate() {
        out[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
    out
}

/// Lit les octets d'une adresse, en-tête d'autorisation facultatif.
pub fn get_bytes(url: &str, token: Option<&str>, extra: &[(&str, &str)]) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut request = ureq::get(url)
        .set("User-Agent", common::USER_AGENT)
        .timeout(std::time::Duration::from_secs(60));
    if let Some(token) = token {
        request = request.set("Authorization", &format!("Bearer {token}"));
    }
    for (key, value) in extra {
        request = request.set(key, value);
    }
    let response = request.call()?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|error| Error::Http(error.to_string()))?;
    Ok(bytes)
}

/// Envoie un corps brut, avec ses en-têtes.
pub fn put_bytes(
    url: &str,
    token: Option<&str>,
    body: Vec<u8>,
    extra: &[(&str, &str)],
) -> Result<()> {
    let mut request = ureq::put(url)
        .set("User-Agent", common::USER_AGENT)
        .timeout(std::time::Duration::from_secs(120));
    if let Some(token) = token {
        request = request.set("Authorization", &format!("Bearer {token}"));
    }
    for (key, value) in extra {
        request = request.set(key, value);
    }
    request.send_bytes(&body)?;
    Ok(())
}

/// POST JSON, réponse JSON (les mutations et lectures GraphQL d'Epic).
pub fn post_json(
    url: &str,
    token: Option<&str>,
    body: &serde_json::Value,
    user_agent: Option<&str>,
) -> Result<serde_json::Value> {
    let mut request = ureq::post(url)
        .set(
            "User-Agent",
            user_agent.unwrap_or(common::USER_AGENT),
        )
        .set("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(60));
    if let Some(token) = token {
        request = request.set("Authorization", &format!("Bearer {token}"));
    }
    let response = request.send_string(&body.to_string())?;
    let text = response
        .into_string()
        .map_err(|error| Error::Http(error.to_string()))?;
    if text.trim().is_empty() {
        return Ok(serde_json::Value::Null);
    }
    serde_json::from_str(&text).map_err(|error| Error::Parse(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_dates_become_unix_seconds() {
        assert_eq!(parse_iso8601("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso8601("2026-06-12T09:17:00.123Z"), Some(1_781_255_820));
        // Un décalage explicite ramène l'heure à UTC.
        assert_eq!(
            parse_iso8601("2026-06-12T11:17:00+02:00"),
            parse_iso8601("2026-06-12T09:17:00Z")
        );
        assert_eq!(parse_iso8601("pas une date"), None);
    }

    #[test]
    fn a_unix_stamp_writes_back_as_iso() {
        let stamp = 1_781_255_820;
        assert_eq!(format_iso8601(stamp), "2026-06-12T09:17:00Z");
        assert_eq!(parse_iso8601(&format_iso8601(stamp)), Some(stamp));
        assert_eq!(format_iso8601(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn md5_matches_the_reference_vectors() {
        // Vecteurs de la RFC 1321.
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            md5_hex(b"The quick brown fox jumps over the lazy dog"),
            "9e107d9d372bb6826bd81d3542a419d6"
        );
        // Un message plus long que soixante-quatre octets passe par plusieurs
        // blocs.
        assert_eq!(md5_hex(b"message digest"), "f96b697d7cb7938d525a2f31aaf161d0");
        assert_eq!(
            md5_hex(b"abcdefghijklmnopqrstuvwxyz"),
            "c3fcd3d76192e4007dfb496cca67e13b"
        );
    }

    #[test]
    fn gzip_keeps_the_same_body_for_the_same_content() {
        let first = gzip_md5(b"sauvegarde").expect("compression");
        let second = gzip_md5(b"sauvegarde").expect("compression");
        // Pas d'horodatage dans l'en-tête : deux envois identiques se
        // ressemblent, sinon GOG verrait un fichier neuf à chaque fois.
        assert_eq!(first.0, second.0);
        assert_eq!(first.1, second.1);
        assert_eq!(first.1.len(), 32);
        assert_ne!(first.1, gzip_md5(b"autre").expect("compression").1);
    }
}
