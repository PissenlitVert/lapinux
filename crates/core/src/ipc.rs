//! Protocole IPC (JSON, trames préfixées par leur longueur) sur socket Unix.

use crate::config::Config;
use crate::error::{Error, Result};
use crate::model::{GameSettings, SessionInfo};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

pub const MAX_FRAME: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Req {
    Ping,
    Scan,
    Games,
    Game { appid: u32 },
    SaveSettings { appid: u32, settings: GameSettings },
    Launch { appid: u32, dry_run: bool },
    Stop,
    Session,
    Jobs,
    InstallGame { appid: u32 },
    CancelJob { id: String },
    Protons,
    ProtonReleases,
    InstallProton { repo: String, asset: String, checksum_url: Option<String> },
    RemoveProton { name: String },
    Doctor,
    OverlayToggle,
    OverlayAction { action: String },
    Devices,
    Profiles,
    Config,
    SetConfig { config: Config },
    Tools,
    InstallTools { which: Vec<String> },
    SteamLoginStart { username: String },
    SteamLoginSubmit { secret: String },
    Stats,
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Envelope {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub data: serde_json::Value,
}

impl Envelope {
    pub fn ok<T: Serialize>(value: &T) -> Self {
        Self {
            ok: true,
            error: None,
            data: serde_json::to_value(value).unwrap_or(serde_json::Value::Null),
        }
    }

    pub fn unit() -> Self {
        Self {
            ok: true,
            error: None,
            data: serde_json::Value::Null,
        }
    }

    pub fn err(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(message.into()),
            data: serde_json::Value::Null,
        }
    }

    pub fn into_data<T: DeserializeOwned>(self) -> Result<T> {
        if !self.ok {
            return Err(Error::Other(
                self.error.unwrap_or_else(|| "erreur inconnue".into()),
            ));
        }
        serde_json::from_value(self.data).map_err(Error::Json)
    }

    pub fn session_hint(&self) -> Option<SessionInfo> {
        serde_json::from_value(self.data.clone()).ok()
    }
}

pub fn write_frame<W: Write>(w: &mut W, payload: &[u8]) -> std::io::Result<()> {
    if payload.len() > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "trame trop grande",
        ));
    }
    w.write_all(&(payload.len() as u32).to_le_bytes())?;
    w.write_all(payload)?;
    w.flush()
}

pub fn read_frame<R: Read>(r: &mut R) -> std::io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "trame trop grande",
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

pub fn send_request(socket: &Path, req: &Req) -> Result<Envelope> {
    send_request_timeout(socket, req, Duration::from_secs(30))
}

pub fn send_request_timeout(socket: &Path, req: &Req, timeout: Duration) -> Result<Envelope> {
    let mut stream = UnixStream::connect(socket)
        .map_err(|e| Error::Ipc(format!("{} : {e}", socket.display())))?;
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(timeout)).ok();
    let payload = serde_json::to_vec(req)?;
    write_frame(&mut stream, &payload).map_err(|e| Error::Ipc(e.to_string()))?;
    let reply = read_frame(&mut stream).map_err(|e| Error::Ipc(e.to_string()))?;
    let env: Envelope = serde_json::from_slice(&reply)?;
    Ok(env)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn frame_roundtrip() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"bonjour").unwrap();
        let mut cursor = Cursor::new(buf);
        assert_eq!(read_frame(&mut cursor).unwrap(), b"bonjour");
    }

    #[test]
    fn oversized_frame_is_rejected() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&(u32::MAX).to_le_bytes());
        let mut cursor = Cursor::new(buf);
        assert!(read_frame(&mut cursor).is_err());
    }

    #[test]
    fn envelope_roundtrip() {
        let env = Envelope::ok(&vec![1u32, 2, 3]);
        let text = serde_json::to_string(&env).unwrap();
        let back: Envelope = serde_json::from_str(&text).unwrap();
        assert_eq!(back.into_data::<Vec<u32>>().unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn envelope_error() {
        let env = Envelope::err("boum");
        assert!(env.clone().into_data::<()>().is_err());
        assert_eq!(env.error.as_deref(), Some("boum"));
    }
}
