//! Client IPC : parle au daemon et le démarre au besoin.

use crate::error::{Error, Result};
use crate::ipc::{self, Envelope, Req};
use crate::paths::LudusPaths;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct Client {
    pub socket: PathBuf,
    pub auto_spawn: bool,
    pub log_file: PathBuf,
}

impl Client {
    pub fn new(paths: &LudusPaths) -> Self {
        Self {
            socket: paths.socket.clone(),
            auto_spawn: true,
            log_file: paths.log_dir.join("daemon.log"),
        }
    }

    pub fn without_spawn(mut self) -> Self {
        self.auto_spawn = false;
        self
    }

    pub fn request(&self, req: &Req) -> Result<Envelope> {
        self.request_with_timeout(req, Duration::from_secs(120))
    }

    pub fn request_with_timeout(&self, req: &Req, timeout: Duration) -> Result<Envelope> {
        match ipc::send_request_timeout(&self.socket, req, timeout) {
            Ok(env) => Ok(env),
            Err(first) => {
                if !self.auto_spawn {
                    return Err(first);
                }
                self.spawn_daemon()?;
                ipc::send_request_timeout(&self.socket, req, timeout).map_err(|e| {
                    Error::Ipc(format!(
                        "daemon injoignable après démarrage ({e}) ; journal : {}",
                        self.log_file.display()
                    ))
                })
            }
        }
    }

    /// Démarre `ludus --daemon` en arrière-plan et attend que le socket réponde.
    pub fn spawn_daemon(&self) -> Result<()> {
        if self.daemon_alive() {
            return Ok(());
        }
        let exe = daemon_binary()?;
        if let Some(parent) = self.log_file.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_file)
            .map_err(|e| Error::Ipc(format!("journal du daemon : {e}")))?;
        let err = log.try_clone().map_err(Error::Io)?;
        Command::new(&exe)
            .arg("--daemon")
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(err))
            .spawn()
            .map_err(|e| Error::Ipc(format!("lancement du daemon ({}) : {e}", exe.display())))?;

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if self.daemon_alive() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err(Error::Ipc(format!(
            "le daemon n'a pas répondu ; journal : {}",
            self.log_file.display()
        )))
    }

    pub fn daemon_alive(&self) -> bool {
        ipc::send_request_timeout(&self.socket, &Req::Ping, Duration::from_millis(400)).is_ok()
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket
    }
}

fn daemon_binary() -> Result<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        // Le binaire courant est `ludus` (ou le test) : on préfère `ludus` à côté.
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("ludus");
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
        return Ok(exe);
    }
    crate::paths::which("ludus").ok_or_else(|| Error::Ipc("binaire ludus introuvable".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_spawn_on_missing_socket() {
        let dir = tempfile::tempdir().unwrap();
        let client = Client {
            socket: dir.path().join("absent.sock"),
            auto_spawn: false,
            log_file: dir.path().join("log"),
        };
        let err = client.request(&Req::Ping).unwrap_err();
        assert!(matches!(err, Error::Ipc(_)));
        assert!(!client.daemon_alive());
    }
}
