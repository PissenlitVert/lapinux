//! Trames du protocole CM : longueur, magie `VT01`, en-têtes struct et proto.

use super::Error;
use std::fmt;
use std::io::{Read, Write};
use std::net::{SocketAddr, ToSocketAddrs};
use std::str::FromStr;

/// Magie de chaque trame TCP du réseau Steam.
pub const MAGIC: &[u8; 4] = b"VT01";
/// Un CM n'envoie jamais de trame de cette taille ; borne de sécurité.
pub const MAX_PACKET: usize = 1 << 20;
/// Bit qui distingue un message protobuf d'un message à en-tête struct.
pub const PROTO_BIT: u32 = 0x8000_0000;

/// Adresse d'un serveur CM.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
}

impl Endpoint {
    pub fn socket_addr(&self) -> Result<SocketAddr, Error> {
        let mut addresses = (self.host.as_str(), self.port)
            .to_socket_addrs()
            .map_err(|e| Error::Connect(format!("{self} : {e}")))?;
        addresses
            .next()
            .ok_or_else(|| Error::Connect(format!("{self} : aucune adresse")))
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.host, self.port)
    }
}

impl FromStr for Endpoint {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (host, port) = value
            .rsplit_once(':')
            .ok_or_else(|| Error::Connect(format!("« {value} » n'est pas host:port")))?;
        let host = host.trim_start_matches('[').trim_end_matches(']');
        let port = port
            .parse()
            .map_err(|_| Error::Connect(format!("port invalide dans « {value} »")))?;
        if host.is_empty() {
            return Err(Error::Connect(format!("hôte vide dans « {value} »")));
        }
        Ok(Self {
            host: host.to_string(),
            port,
        })
    }
}

/// Lit une trame et renvoie son contenu, magie retirée.
///
/// La longueur compte le message seul : `[longueur][VT01][message]`, où
/// `longueur` vaut la taille de `message`. Le message peut être chiffré ; la
/// magie et la longueur, elles, restent en clair.
pub fn read_packet<R: Read>(reader: &mut R) -> Result<Vec<u8>, Error> {
    let mut head = [0u8; 4];
    reader.read_exact(&mut head)?;
    let len = u32::from_le_bytes(head) as usize;
    if !(1..=MAX_PACKET).contains(&len) {
        return Err(Error::Protocol(format!(
            "taille de trame invalide : {len} (octets {head:02x?})"
        )));
    }
    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(Error::Protocol(format!("magie inattendue : {magic:02x?}")));
    }
    let mut data = vec![0u8; len];
    reader.read_exact(&mut data)?;
    Ok(data)
}

/// Écrit une trame (longueur du message, magie, message).
pub fn write_packet<W: Write>(writer: &mut W, payload: &[u8]) -> Result<(), Error> {
    let len = u32::try_from(payload.len())
        .map_err(|_| Error::Protocol("trame trop grande".into()))?;
    writer.write_all(&len.to_le_bytes())?;
    writer.write_all(MAGIC)?;
    writer.write_all(payload)?;
    writer.flush()?;
    Ok(())
}

/// Message à en-tête `MsgHdr` : EMsg, puis deux identifiants de job à -1.
pub fn struct_message(emsg: u32, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(20 + body.len());
    out.extend_from_slice(&emsg.to_le_bytes());
    out.extend_from_slice(&u64::MAX.to_le_bytes());
    out.extend_from_slice(&u64::MAX.to_le_bytes());
    out.extend_from_slice(body);
    out
}

/// Décode un message `MsgHdr` et renvoie `(emsg, corps)`.
pub fn parse_struct(message: &[u8]) -> Option<(u32, &[u8])> {
    if message.len() < 20 {
        return None;
    }
    let emsg = u32::from_le_bytes(message[..4].try_into().ok()?);
    Some((emsg, &message[20..]))
}

/// Message protobuf : EMsg avec son bit, longueur d'en-tête, en-tête, corps.
pub fn proto_message(emsg: u32, header: &[u8], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + header.len() + body.len());
    out.extend_from_slice(&(emsg | PROTO_BIT).to_le_bytes());
    out.extend_from_slice(&(header.len() as u32).to_le_bytes());
    out.extend_from_slice(header);
    out.extend_from_slice(body);
    out
}

/// Décode un message protobuf et renvoie `(emsg, en-tête, corps)`.
pub fn parse_proto(message: &[u8]) -> Option<(u32, &[u8], &[u8])> {
    if message.len() < 8 {
        return None;
    }
    let raw = u32::from_le_bytes(message[..4].try_into().ok()?);
    let len = u32::from_le_bytes(message[4..8].try_into().ok()?) as usize;
    let end = 8usize.checked_add(len)?;
    if end > message.len() {
        return None;
    }
    Some((raw & !PROTO_BIT, &message[8..end], &message[end..]))
}

/// Identifiant d'un message, sans le bit protobuf.
pub fn message_id(message: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(message.get(..4)?.try_into().ok()?) & !PROTO_BIT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn frame_round_trip() {
        let mut buffer = Vec::new();
        write_packet(&mut buffer, b"bonjour").unwrap();
        assert_eq!(&buffer[..4], &7u32.to_le_bytes());
        let mut cursor = Cursor::new(buffer);
        assert_eq!(read_packet(&mut cursor).unwrap(), b"bonjour");
    }

    #[test]
    fn frame_length_excludes_magic() {
        // Forme exacte observée sur un CM : 44 = 4 (EMsg) + 16 (en-tête) + 24 (corps).
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&44u32.to_le_bytes());
        buffer.extend_from_slice(b"VT01");
        buffer.extend_from_slice(&[0x17, 0x05, 0x00, 0x00]);
        buffer.extend_from_slice(&[0xff; 16]);
        buffer.extend_from_slice(&[0x01; 24]);
        let packet = read_packet(&mut Cursor::new(buffer)).unwrap();
        assert_eq!(packet.len(), 44);
        assert_eq!(&packet[..4], &[0x17, 0x05, 0x00, 0x00]);
    }

    #[test]
    fn frame_rejects_bad_magic() {
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&9u32.to_le_bytes());
        buffer.extend_from_slice(b"XXXX");
        buffer.extend_from_slice(b"corps");
        assert!(read_packet(&mut Cursor::new(buffer)).is_err());
    }

    #[test]
    fn frame_rejects_oversized_length() {
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&((MAX_PACKET + 1) as u32).to_le_bytes());
        assert!(read_packet(&mut Cursor::new(buffer)).is_err());
    }

    #[test]
    fn frame_rejects_zero_length() {
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&0u32.to_le_bytes());
        assert!(read_packet(&mut Cursor::new(buffer)).is_err());
    }

    #[test]
    fn struct_header_is_twenty_bytes() {
        let message = struct_message(1303, &[1, 2, 3]);
        let (emsg, body) = parse_struct(&message).unwrap();
        assert_eq!(emsg, 1303);
        assert_eq!(body, &[1, 2, 3]);
    }

    #[test]
    fn struct_rejects_truncated_message() {
        assert!(parse_struct(&[1, 2, 3]).is_none());
    }

    #[test]
    fn proto_header_round_trip() {
        let message = proto_message(5514, &[0x09, 0x11], &[0x08, 0x01]);
        let (emsg, header, body) = parse_proto(&message).unwrap();
        assert_eq!(emsg, 5514);
        assert_eq!(header, &[0x09, 0x11]);
        assert_eq!(body, &[0x08, 0x01]);
        assert_eq!(message_id(&message), Some(5514));
    }

    #[test]
    fn proto_rejects_truncated_header() {
        let message = proto_message(1, &[9, 9, 9], &[]);
        assert!(parse_proto(&message[..9]).is_none());
    }

    #[test]
    fn endpoint_parsing() {
        let endpoint: Endpoint = "cm2-fra1.steamserver.net:27017".parse().unwrap();
        assert_eq!(endpoint.host, "cm2-fra1.steamserver.net");
        assert_eq!(endpoint.port, 27017);
        assert_eq!(endpoint.to_string(), "cm2-fra1.steamserver.net:27017");
        assert!("sans-port".parse::<Endpoint>().is_err());
        assert!(":27017".parse::<Endpoint>().is_err());
        assert!("hote:port".parse::<Endpoint>().is_err());
    }
}
