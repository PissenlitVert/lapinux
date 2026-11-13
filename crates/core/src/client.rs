//! Client IPC : parle au daemon et le démarre au besoin.

use crate::error::{Error, Result};
use crate::ipc::{self, DaemonBuild, Envelope, Req};
use crate::paths::LapinuxPaths;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct Client {
    pub socket: PathBuf,
    pub auto_spawn: bool,
    pub log_file: PathBuf,
}

impl Client {
    pub fn new(paths: &LapinuxPaths) -> Self {
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

    /// Copie détachée pour un fil de fond : mêmes socket, journal et mode de
    /// démarrage que ce client.
    pub fn detached(&self) -> Self {
        Self {
            socket: self.socket.clone(),
            auto_spawn: self.auto_spawn,
            log_file: self.log_file.clone(),
        }
    }

    /// Copie qui ne démarre jamais le daemon : une lecture de fond ne doit pas
    /// réveiller le service.
    pub fn detached_quiet(&self) -> Self {
        self.detached().without_spawn()
    }

    pub fn request(&self, req: &Req) -> Result<Envelope> {
        self.request_with_timeout(req, Duration::from_secs(120))
    }

    pub fn request_with_timeout(&self, req: &Req, timeout: Duration) -> Result<Envelope> {
        match ipc::send_request_timeout(&self.socket, req, timeout) {
            Ok(env) => Ok(env),
            Err(first) => {
                // Un délai de lecture n'est pas une panne : le daemon écoute
                // toujours, il n'a simplement pas fini. Le relancer ici
                // lancerait un second processus qui ne pourrait pas reprendre
                // le socket, et la vraie réponse serait perdue.
                if !self.auto_spawn || matches!(&first, Error::IpcTimeout(_)) {
                    return Err(first);
                }
                self.spawn_daemon()?;
                ipc::send_request_timeout(&self.socket, req, timeout).map_err(|e| {
                    if matches!(&e, Error::IpcTimeout(_)) {
                        Error::IpcTimeout(format!(
                            "le daemon n'a pas répondu à temps après son démarrage ({e}) ; journal : {}",
                            self.log_file.display()
                        ))
                    } else {
                        Error::Ipc(format!(
                            "daemon injoignable après démarrage ({e}) ; journal : {}",
                            self.log_file.display()
                        ))
                    }
                })
            }
        }
    }

    /// Démarre `lapinux --daemon` en arrière-plan et attend que le socket réponde.
    ///
    /// Un seul appelant démarre à la fois : l'interface, le tray et une page
    /// peuvent réclamer le daemon au même instant, et les perdants lançaient
    /// chacun un processus qui scannait la bibliothèque avant d'échouer sur le
    /// verrou de la base (« Database already open »).
    pub fn spawn_daemon(&self) -> Result<()> {
        if self.daemon_alive() {
            return Ok(());
        }
        let _spawn = SpawnLock::acquire(&self.socket, Duration::from_secs(6));
        // Le premier appelant a pu démarrer le daemon pendant l'attente.
        if self.daemon_alive() {
            return Ok(());
        }
        let exe = daemon_binary()?;
        if let Some(parent) = self.log_file.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        // Le journal du daemon est rouvert à chaque démarrage : au-delà de la
        // limite, il part en `.1` plutôt que de garder des mois d'historique.
        crate::util::rotate_log(&self.log_file);
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
        match ipc::send_request_timeout(&self.socket, &Req::Ping, Duration::from_millis(400)) {
            Ok(_) => true,
            // Le socket a accepté la connexion : un daemon écoute, même s'il
            // est momentanément occupé. Le déclarer mort ferait lancer un
            // second processus, qui échouerait sur le socket déjà lié.
            Err(Error::IpcTimeout(_)) => true,
            Err(_) => false,
        }
    }

    /// Identité du daemon qui tourne : version, empreinte du binaire et date
    /// de démarrage. `None` quand personne n'écoute sur le socket.
    pub fn daemon_build(&self) -> Option<DaemonBuild> {
        let envelope =
            ipc::send_request_timeout(&self.socket, &Req::Ping, Duration::from_millis(600)).ok()?;
        // Un daemon d'avant l'empreinte répond sans données : son empreinte à
        // zéro suffit à le reconnaître comme ancien, et le socket ne ment pas
        // en le faisant passer pour arrêté.
        Some(envelope.into_data().unwrap_or_default())
    }

    /// Remplace un daemon lancé depuis un binaire plus ancien que celui de
    /// l'application. Un daemon antérieur à une mise à jour ignore les champs
    /// de configuration qu'il ne connaît pas et les efface en réenregistrant
    /// le fichier : sans ce contrôle, un réglage ajouté ne tient pas d'un
    /// lancement à l'autre. Renvoie vrai si un redémarrage a eu lieu.
    pub fn restart_stale_daemon(&self) -> bool {
        let Some(running) = self.daemon_build() else {
            // Aucun daemon : il sera démarré au premier rafraîchissement.
            return false;
        };
        let Some(current) = daemon_binary_date() else {
            return false;
        };
        // Un daemon trop ancien pour publier son empreinte répond zéro : il
        // est forcément antérieur à ce binaire.
        if is_current(running.stamp, current) {
            return false;
        }
        self.replace_daemon()
    }

    /// Arrête le daemon et en relance un depuis le binaire de l'application,
    /// sans condition. Renvoie vrai si un daemon répond à la fin.
    pub fn restart_daemon(&self) -> bool {
        self.replace_daemon()
    }

    fn replace_daemon(&self) -> bool {
        // Les anciennes versions ne comprennent pas cette requête et ne sont
        // donc jamais arrêtées par un client qui ne peut vérifier leur sécurité.
        if !matches!(ipc::send_request_timeout(&self.socket, &Req::ShutdownIfIdle, Duration::from_secs(2)), Ok(response) if response.ok) {
            return false;
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline && self.daemon_alive() {
            std::thread::sleep(Duration::from_millis(50));
        }
        // Le daemon retire son socket avant de libérer sa base : sans cette
        // pause, le suivant échoue sur le verrou et ne se relève jamais.
        std::thread::sleep(Duration::from_millis(400));
        for _ in 0..2 {
            if self.spawn_daemon().is_ok() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        false
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket
    }
}

/// Verrou de démarrage du daemon, posé à côté de son socket.
///
/// Il ne remplace pas le verrou de la base : il évite d'en arriver là. Un
/// appelant qui n'obtient pas le verrou attend que le gagnant ait démarré le
/// daemon, puis le trouve vivant au lieu de lancer un second processus.
struct SpawnLock(std::fs::File);

impl SpawnLock {
    /// `None` si le verrou reste pris au bout du délai : l'appelant démarre
    /// alors sans lui, comme avant.
    fn acquire(socket: &Path, timeout: Duration) -> Option<Self> {
        let path = socket.with_file_name("daemon-spawn.lock");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .ok()?;
        let fd = {
            use std::os::unix::io::AsRawFd;
            file.as_raw_fd()
        };
        let deadline = Instant::now() + timeout;
        loop {
            let acquired = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } == 0;
            if acquired {
                return Some(Self(file));
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

impl Drop for SpawnLock {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

/// Date du binaire qui sert de daemon, en millisecondes Unix.
pub fn daemon_binary_date() -> Option<u64> {
    daemon_binary()
        .ok()
        .and_then(|path| crate::paths::binary_stamp(&path))
}

/// Le daemon qui tourne vient-il du binaire présent sur le disque ? Un daemon
/// qui ne publie pas d'empreinte (zéro) est toujours considéré comme ancien.
pub fn is_current(running: u64, binary: u64) -> bool {
    running > 0 && running >= binary
}

pub fn daemon_binary() -> Result<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        // Le binaire courant est `lapinux` (ou un binaire de test) : on
        // préfère celui de l'application, juste à côté.
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(crate::APP_BIN);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
        return Ok(exe);
    }
    crate::paths::which(crate::APP_BIN)
        .ok_or_else(|| Error::Ipc(format!("binaire {} introuvable", crate::APP_BIN)))
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

    #[test]
    fn no_restart_without_a_daemon() {
        let dir = tempfile::tempdir().unwrap();
        let client = Client {
            socket: dir.path().join("absent.sock"),
            auto_spawn: false,
            log_file: dir.path().join("log"),
        };
        // Sans daemon, rien à remplacer : le démarrage reste paresseux.
        assert!(!client.restart_stale_daemon());
    }

    #[test]
    fn an_older_daemon_is_not_the_binary_on_disk() {
        assert!(is_current(200, 100));
        assert!(is_current(100, 100));
        assert!(!is_current(99, 100));
        // Pas d'empreinte : le daemon est d'avant ce protocole.
        assert!(!is_current(0, 100));
    }

    #[test]
    fn a_build_holds_the_version_and_startup_date() {
        let build = DaemonBuild {
            stamp: 7,
            version: "0.1.0".into(),
            started: 1_700_000_000,
        };
        let json = serde_json::to_string(&build).unwrap();
        assert_eq!(serde_json::from_str::<DaemonBuild>(&json).unwrap(), build);
        // Un daemon d'avant ces champs reste lisible.
        let old: DaemonBuild = serde_json::from_str("{\"stamp\":42}").unwrap();
        assert_eq!(old.stamp, 42);
        assert!(old.version.is_empty());
        assert_eq!(old.started, 0);
    }

    #[test]
    fn a_daemon_without_a_build_stamp_is_replaced() {
        // Le protocole d'avant l'empreinte répondait un `Ping` sans données :
        // c'est exactement le cas d'un daemon resté sur une version antérieure.
        let old = Envelope::unit();
        assert!(old.data.get("stamp").and_then(|v| v.as_u64()).is_none());
        let fresh = Envelope::ok(&ipc::DaemonBuild {
            stamp: 42,
            ..Default::default()
        });
        assert_eq!(fresh.data.get("stamp").and_then(|v| v.as_u64()), Some(42));
    }

    #[test]
    fn the_spawn_lock_serialises_who_starts_the_daemon() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("control.sock");
        let first = SpawnLock::acquire(&socket, Duration::from_millis(100)).expect("verrou libre");
        // Le suivant attend : sans ce verrou, deux daemons partaient de front.
        assert!(SpawnLock::acquire(&socket, Duration::from_millis(150)).is_none());
        drop(first);
        assert!(SpawnLock::acquire(&socket, Duration::from_millis(150)).is_some());
    }

    #[test]
    fn restart_never_uses_unsafe_shutdown_on_an_old_daemon() {
        use std::os::unix::net::UnixListener;
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("control.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let payload = ipc::read_frame(&mut stream).unwrap();
            let req: Req = serde_json::from_slice(&payload).unwrap();
            assert_eq!(req, Req::ShutdownIfIdle);
            let body = serde_json::to_vec(&Envelope::err("unknown request")).unwrap();
            ipc::write_frame(&mut stream, &body).unwrap();
        });
        let client = Client { socket, auto_spawn: false, log_file: dir.path().join("daemon.log") };
        assert!(!client.restart_daemon());
        server.join().unwrap();
    }

    #[test]
    fn a_slow_daemon_is_not_mistaken_for_a_dead_one() {
        use std::os::unix::net::UnixListener;
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("control.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        // Le serveur accepte chaque connexion puis se tait : c'est un daemon
        // occupé, pas un daemon absent. Il reste à l'écoute jusqu'au signal
        // d'arrêt : sur une machine lente, s'arrêter après une seule
        // acceptation faisait répondre « connexion refusée » au contrôle
        // suivant, alors que le daemon était seulement occupé.
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            while stop_rx.try_recv().is_err() {
                match listener.accept() {
                    Ok((_stream, _)) => std::thread::sleep(Duration::from_millis(600)),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        let client = Client {
            socket,
            auto_spawn: true,
            log_file: dir.path().join("daemon.log"),
        };
        let started = Instant::now();
        let error = client
            .request_with_timeout(&Req::Ping, Duration::from_millis(120))
            .unwrap_err();
        // Le délai est rendu tel quel : le client n'a pas lancé un second
        // daemon qui aurait échoué sur le socket déjà lié.
        assert!(matches!(error, Error::IpcTimeout(_)), "{error}");
        assert!(started.elapsed() < Duration::from_secs(1));
        // Un socket qui accepte la connexion reste vivant, même muet.
        assert!(client.daemon_alive());
        let _ = stop_tx.send(());
        server.join().ok();
    }
}
