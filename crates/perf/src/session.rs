//! Composition de la commande finale et supervision de la session de jeu.

use crate::caps::PerfCaps;
use crate::gamescope::{self, GamescopeSettings};
use crate::{Error, Result};
use lapinux_proton::LaunchPlan as Plan;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub struct PerfConfig {
    pub gamescope: GamescopeSettings,
    /// Variables propres au jeu, placées après la scène et le compositeur.
    /// None supprime une variable héritée avant le lancement.
    pub game_env: Vec<(String, Option<String>)>,
    /// Enveloppe le jeu dans `lapinux scene` lorsque GameScope est utilisé.
    pub scene: bool,
    /// Nom affiché par la scène de lancement et de fin.
    pub scene_name: String,
    /// Jaquette locale affichée par la scène. Elle est déjà en cache : la
    /// scène ne fait aucune requête réseau pendant le lancement.
    pub scene_cover: Option<PathBuf>,
    /// Visuel large 16:9 de la scène, quand un fichier local est disponible.
    pub scene_backdrop: Option<PathBuf>,
}

impl Default for PerfConfig {
    fn default() -> Self {
        Self {
            gamescope: GamescopeSettings::default(),
            game_env: Vec::new(),
            scene: false,
            scene_name: String::new(),
            scene_cover: None,
            scene_backdrop: None,
        }
    }
}

pub struct PreparedLaunch {
    pub command: Command,
    pub log_path: PathBuf,
    pub gamescope: bool,
    /// La commande passe par `lapinux scene` dans GameScope.
    pub scene: bool,
    /// Fichier que la scène pose dès que le jeu est terminé. Il permet au
    /// daemon de couper la capture et le HUD avant les quatre secondes de
    /// l'écran de fin.
    pub scene_exit_path: Option<PathBuf>,
    /// Fichier que le daemon pose à la première image du jeu : la scène s'en
    /// sert pour s'effacer derrière le jeu au lieu de le masquer.
    pub scene_start_path: Option<PathBuf>,
    /// Tube des statistiques gamescope : il porte la cadence réelle du jeu.
    pub stats_path: Option<PathBuf>,
    /// Motif pour lequel GameScope a été écarté (absent, trop ancien, option
    /// inconnue). Le lancement direct a alors lieu, sans HUD ni scène :
    /// l'interface peut le dire au joueur au lieu de le laisser deviner.
    pub gamescope_warning: Option<String>,
}

pub struct Session {
    pub appid: u32,
    pub pid: u32,
    /// Préfixe Proton de la partie : ses processus le portent dans leur
    /// environnement, même détachés de la session du jeu.
    compat_data: Option<PathBuf>,
    pub log_path: PathBuf,
    /// Position du journal partagé au démarrage de la session : le guetteur
    /// de la première image reprend là, sans relire les parties précédentes.
    pub log_start: u64,
    /// Tube de statistiques de gamescope, lu par le daemon pendant la partie.
    pub stats_path: Option<PathBuf>,
    /// Fichier de fin déposé par la scène, quand elle est utilisée.
    pub scene_exit_path: Option<PathBuf>,
    /// Fichier de première image, déposé par le daemon pour la scène.
    pub scene_start_path: Option<PathBuf>,
    pub started: Instant,
    child: std::process::Child,
}

pub struct SessionOutcome {
    pub code: Option<i32>,
    pub duration: Duration,
}

pub fn prepare(
    plan: &Plan,
    appid: u32,
    config: &PerfConfig,
    caps: &PerfCaps,
    _config_dir: &std::path::Path,
    log_dir: &std::path::Path,
) -> Result<PreparedLaunch> {
    let env = plan.env.clone();
    let mut program = plan.program.clone();
    let mut args = plan.args.clone();
    let mut use_gamescope = false;
    let mut use_scene = false;
    let mut scene_exit_path: Option<PathBuf> = None;
    let mut scene_start_path: Option<PathBuf> = None;
    let mut stats_path: Option<PathBuf> = None;

    if !config.game_env.is_empty() {
        // `env` est lui-même exécuté comme enfant de la scène / GameScope :
        // les couches restent absentes de ces processus parents.
        let env_bin = lapinux_core::paths::which("env")
            .ok_or_else(|| Error::Other("env executable not found".into()))?;
        let mut wrapped = Vec::new();
        for (key, value) in &config.game_env {
            match value {
                Some(_) => {}
                None => {
                    wrapped.push("-u".to_string());
                    wrapped.push(key.clone());
                }
            }
        }
        wrapped.push("--".to_string());
        for (key, value) in &config.game_env {
            if let Some(value) = value {
                wrapped.push(format!("{key}={value}"));
            }
        }
        wrapped.push(program.to_string_lossy().to_string());
        wrapped.extend(args);
        args = wrapped;
        program = env_bin;
    }

    let gamescope_available = caps.gamescope.clone();
    let mut gamescope_warning: Option<String> = None;
    // Le binaire est interrogé avant d'être utilisé : une version trop
    // ancienne, ou une option qu'il ne connaît pas, ferait sortir GameScope
    // et perdrait la partie au lancement. Le verdict est retenu par version
    // installée, donc un seul `--version` par binaire.
    let gamescope_available = match gamescope_available {
        Some(bin) => match crate::gamescope_check::check(&bin).problem() {
            Some(problem) => {
                tracing::warn!("{problem}");
                gamescope_warning = Some(problem);
                None
            }
            None => Some(bin),
        },
        None => None,
    };
    if config.gamescope.enabled {
        if let Some(gamescope_bin) = gamescope_available {
            use_gamescope = true;
            let mut settings = config.gamescope.clone();
            // Gamescope n'ouvre son tube que si l'autre bout existe déjà : on
            // crée le FIFO ici, le daemon le lit.
            if let Some(path) = create_fifo(log_dir, appid) {
                settings.stats_path = Some(path.to_string_lossy().to_string());
                stats_path = Some(path);
            }
            let mut scope_args = gamescope::args(&settings);
            // La scène doit être un client Wayland de GameScope : c'est le
            // seul moyen pour elle de rester une surface persistante sous le
            // jeu. Le backend Wayland n'est demandé que si le bureau en fournit
            // déjà un ; sinon GameScope garde son backend habituel.
            if config.scene {
                match std::env::current_exe() {
                    Ok(exe) => {
                        use_scene = true;
                        std::fs::create_dir_all(log_dir).ok();
                        let scene_stamp = lapinux_core::util::now_unix();
                        let exit_path = log_dir.join(format!("scene-{appid}-{scene_stamp}.exit"));
                        let start_path = log_dir.join(format!("scene-{appid}-{scene_stamp}.start"));
                        let _ = std::fs::remove_file(&exit_path);
                        let _ = std::fs::remove_file(&start_path);
                        scene_exit_path = Some(exit_path.clone());
                        scene_start_path = Some(start_path.clone());
                        let game_program = program.to_string_lossy().to_string();
                        let game_args = args.clone();
                        let scene_name = if config.scene_name.trim().is_empty() {
                            lapinux_core::APP_NAME.to_string()
                        } else {
                            config.scene_name.clone()
                        };
                        program = exe;
                        // La scène hérite de l'environnement Proton et du
                        // répertoire courant du daemon, puis les transmet au
                        // jeu : la commande du jeu reste inchangée.
                        args = vec![
                            "scene".into(),
                            // La scène doit occuper exactement la surface de
                            // GameScope : laissée à sa taille par défaut, elle
                            // laissait une bande de bureau visible sur le côté.
                            "--width".into(),
                            config.gamescope.width.max(320).to_string(),
                            "--height".into(),
                            config.gamescope.height.max(240).to_string(),
                            "--name".into(),
                            scene_name,
                            "--end-secondes".into(),
                            "4".into(),
                            "--exit-file".into(),
                            exit_path.to_string_lossy().to_string(),
                            "--start-file".into(),
                            start_path.to_string_lossy().to_string(),
                        ];
                        if let Some(backdrop) =
                            config.scene_backdrop.as_ref().filter(|path| path.is_file())
                        {
                            args.push("--backdrop".into());
                            args.push(backdrop.to_string_lossy().to_string());
                        }
                        if let Some(cover) =
                            config.scene_cover.as_ref().filter(|path| path.is_file())
                        {
                            args.push("--cover".into());
                            args.push(cover.to_string_lossy().to_string());
                        }
                        args.push("--".into());
                        args.push(game_program);
                        args.extend(game_args);
                        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                            scope_args.push("--backend".into());
                            scope_args.push("wayland".into());
                            scope_args.push("--expose-wayland".into());
                        }
                    }
                    Err(err) => {
                        tracing::warn!(
                            "scène : binaire Lapinux introuvable, lancement direct ({err})"
                        );
                    }
                }
            }
            // La commande est écrite dans le journal : sans elle, une limite
            // absente ou mal calculée restait invisible.
            tracing::info!("GameScope : {}", scope_args.join(" "));
            scope_args.push("--".into());
            scope_args.push(program.to_string_lossy().to_string());
            scope_args.extend(args.iter().cloned());
            program = gamescope_bin;
            args = scope_args;
        } else if gamescope_warning.is_none() {
            let message = "GameScope demandé mais introuvable : lancement direct".to_string();
            tracing::warn!("{message}");
            gamescope_warning = Some(message);
        }
    }

    let mut command = Command::new(&program);
    command.args(&args);
    // Les variables du plan sont appliquées avant les protections : elles ne
    // doivent pas réactiver une couche dans GameScope ou la scène.
    for (key, value) in &env {
        command.env(key, value);
    }
    // Un profil lsfg-vk éventuellement actif sur le système ne doit pas
    // charger la couche dans GameScope ou dans la scène ; `env -u` l'enlève
    // ensuite dans le seul arbre de processus du jeu.
    if config
        .game_env
        .iter()
        .any(|(key, _)| key == "DISABLE_LSFGVK")
    {
        command.env("DISABLE_LSFGVK", "1");
    }
    if config.game_env.iter().any(|(key, _)| key == "DISABLE_AFMF") {
        command.env("DISABLE_AFMF", "1");
    }
    for key in [
        "LD_PRELOAD",
        "NVPRESENT_ENABLE_SMOOTH_MOTION",
        "PROTON_USE_OPTISCALER",
        "ENABLE_VKBASALT",
        "VKBASALT_CONFIG_FILE",
        "AFMF_ENABLE",
        "AFMF_GAMESCOPE",
        "AFMF_PERFORMANCE_MODE",
        "AFMF_SEARCH_MODE",
        "VK_ADD_IMPLICIT_LAYER_PATH",
        "WINEDLLOVERRIDES",
        "PROTON_USE_WINED3D",
        "PROTON_DXVK_D3D8",
        "PROTON_OLD_GL_STRING",
        "PROTON_NO_FSYNC",
        "PROTON_NO_ESYNC",
        "PROTON_NO_NTSYNC",
        "DXVK_CONFIG",
        "VKD3D_CONFIG",
    ] {
        if config.game_env.iter().any(|(name, _)| name == key) {
            command.env_remove(key);
        }
    }
    // Le repli en rendu logiciel ne vaut que pour la fenêtre de Lapinux : les
    // variables qui forcent Mesa ne doivent pas descendre jusqu'au jeu. Sinon
    // Proton, DXVK et GameScope perdent le GPU, et la partie ne démarre plus
    // (ou démarre sur llvmpipe, à quelques images par seconde). Une variable
    // posée explicitement dans les réglages du jeu, elle, reste respectée.
    for key in [
        "__EGL_VENDOR_LIBRARY_FILENAMES",
        "__GLX_VENDOR_LIBRARY_NAME",
        "LIBGL_ALWAYS_SOFTWARE",
        "GALLIUM_DRIVER",
    ] {
        if !env.iter().any(|(name, _)| name == key) {
            command.env_remove(key);
        }
    }
    command.env("LAPINUX_APPID", appid.to_string());
    if let Some(cwd) = &plan.cwd {
        command.current_dir(cwd);
    }
    std::fs::create_dir_all(log_dir).map_err(Error::Io)?;
    // Un journal par partie : la carte d'activité ne montre que la sortie de
    // cette session, et supprimer la partie supprime son fichier sans toucher
    // aux autres. L'horodatage évite qu'un même jeu écrase sa partie d'avant.
    let log_path = log_dir.join(format!(
        "game-{appid}-{}.log",
        lapinux_core::util::now_unix()
    ));
    command.stdout(Stdio::null()).stderr(Stdio::null());

    Ok(PreparedLaunch {
        command,
        log_path,
        gamescope: use_gamescope,
        scene: use_scene,
        scene_exit_path,
        scene_start_path,
        stats_path,
        gamescope_warning,
    })
}

/// Chemin du préfixe Proton inscrit dans la commande.
fn compat_data(command: &Command) -> Option<PathBuf> {
    command.get_envs().find_map(|(key, value)| {
        if key != "STEAM_COMPAT_DATA_PATH" {
            return None;
        }
        value.map(PathBuf::from)
    })
}

/// Tue les processus Proton restés attachés au préfixe.
///
/// `wineserver`, ses `winedevice.exe` et le `gamescopereaper` de GameScope
/// quittent la session du jeu : le signal envoyé au groupe les laisse en vie.
/// Ils gardent alors le préfixe occupé, et le lancement suivant peut rester
/// noir après le splash. L'environnement est le seul lien sûr : chaque
/// processus Proton le porte encore, préfixe compris.
pub fn reap_prefix(prefix: &std::path::Path) {
    use std::os::unix::ffi::OsStrExt;

    let mut marker = std::ffi::OsString::from("STEAM_COMPAT_DATA_PATH=");
    marker.push(prefix);
    let marker = marker.as_os_str().as_bytes();
    let me = std::process::id() as i32;
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return;
    };
    let mut victims = Vec::new();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i32>().ok())
        else {
            continue;
        };
        // Le daemon lui-même n'a jamais le préfixe dans son environnement :
        // il ne le transmet qu'aux parties lancées. On s'écarte quand même.
        if pid <= 1 || pid == me {
            continue;
        }
        let Ok(environ) = std::fs::read(format!("/proc/{pid}/environ")) else {
            continue;
        };
        if environ
            .split(|byte| *byte == 0)
            .any(|variable| variable == marker)
        {
            victims.push(pid);
        }
    }
    if victims.is_empty() {
        return;
    }
    tracing::info!(
        "session : {} processus Proton restés attachés au préfixe, nettoyés",
        victims.len()
    );
    for pid in &victims {
        unsafe {
            libc::kill(*pid, libc::SIGTERM);
        }
    }
    for _ in 0..20 {
        if victims
            .iter()
            .all(|pid| unsafe { libc::kill(*pid, 0) } != 0)
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    for pid in &victims {
        unsafe {
            libc::kill(*pid, libc::SIGKILL);
        }
    }
}

/// Crée le tube de statistiques d'une partie. Un reliquat d'une session tuée
/// serait ouvert sans lecteur : il est remplacé.
fn create_fifo(log_dir: &std::path::Path, appid: u32) -> Option<PathBuf> {
    std::fs::create_dir_all(log_dir).ok()?;
    let path = log_dir.join(format!(
        "fps-{appid}-{}.fifo",
        lapinux_core::util::now_unix()
    ));
    let _ = std::fs::remove_file(&path);
    let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
    // 0600 : le tube reste dans le dossier de l'utilisateur.
    if unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) } != 0 {
        tracing::warn!("tube de statistiques : {}", std::io::Error::last_os_error());
        return None;
    }
    Some(path)
}

impl Session {
    pub fn spawn(appid: u32, mut prepared: PreparedLaunch) -> Result<Self> {
        use std::os::unix::process::CommandExt;
        lapinux_core::util::rotate_log(&prepared.log_path);
        let mut log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&prepared.log_path)
            .map_err(Error::Io)?;
        {
            use std::io::Write;
            let _ = writeln!(
                log,
                "\n=== session {appid} · {} ===",
                lapinux_core::util::format_datetime(lapinux_core::util::now_unix())
            );
        }
        let log_start = log.metadata().map(|meta| meta.len()).unwrap_or(0);
        let log_err = log.try_clone().map_err(Error::Io)?;
        prepared.command.stdout(Stdio::from(log));
        prepared.command.stderr(Stdio::from(log_err));
        unsafe {
            prepared.command.pre_exec(|| {
                // Le service du daemon tourne avec `Nice=5` pour ne pas
                // concurrencer le bureau pendant les téléchargements. Le jeu
                // héritait de cette priorité basse : on la ramène à la
                // normale avant l'exécution, sans toucher au reste du daemon.
                let _ = libc::setpriority(libc::PRIO_PROCESS, 0, 0);
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let compat_data = compat_data(&prepared.command);
        // Un reliquat d'une partie précédente garde le préfixe occupé : on le
        // retire avant que Proton n'y entre, sinon le jeu peut ne rien peindre
        // du tout et rester noir derrière son splash.
        if let Some(prefix) = compat_data.as_deref() {
            reap_prefix(prefix);
        }
        let child = prepared.command.spawn().map_err(Error::Io)?;
        let pid = child.id();
        Ok(Self {
            appid,
            pid,
            compat_data,
            log_path: prepared.log_path,
            log_start,
            scene_exit_path: prepared.scene_exit_path,
            scene_start_path: prepared.scene_start_path,
            stats_path: prepared.stats_path,
            started: Instant::now(),
            child,
        })
    }

    pub fn try_wait(&mut self) -> Result<Option<SessionOutcome>> {
        match self.child.try_wait().map_err(Error::Io)? {
            Some(status) => Ok(Some(SessionOutcome {
                code: status.code(),
                duration: self.started.elapsed(),
            })),
            None => Ok(None),
        }
    }

    pub fn wait(&mut self) -> Result<SessionOutcome> {
        let status = self.child.wait().map_err(Error::Io)?;
        Ok(SessionOutcome {
            code: status.code(),
            duration: self.started.elapsed(),
        })
    }

    /// Envoie SIGTERM au groupe de processus puis SIGKILL après un délai.
    pub fn terminate(&mut self) {
        let pid = self.pid as i32;
        unsafe {
            libc::kill(-pid, libc::SIGTERM);
        }
        for _ in 0..20 {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
        self.reap_child();
        self.reap_leftovers();
    }

    /// Récolte le processus de session. `SIGKILL` tue mais ne récolte pas :
    /// tant que personne n'appelle `wait`, GameScope reste zombie et son
    /// `/proc/<pid>` survit. Les guetteurs qui attendent sa disparition
    /// (restauration du pont Steamworks, fin de session) croient alors la
    /// partie encore en cours, et ne font jamais leur travail.
    fn reap_child(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Ok(None) => {
                    tracing::warn!(
                        "session : processus {} toujours vivant après SIGKILL, récolte abandonnée",
                        self.pid
                    );
                    return;
                }
                Err(error) => {
                    tracing::warn!(
                        "session : récolte du processus {} impossible ({error})",
                        self.pid
                    );
                    return;
                }
            }
        }
    }

    /// Préfixe Proton de la partie : ses processus le portent dans leur
    /// environnement, ce qui permet de les retrouver même détachés.
    pub fn compat_data(&self) -> Option<&std::path::Path> {
        self.compat_data.as_deref()
    }

    /// Retire les processus Proton que la partie laisse dans son préfixe.
    /// Appelé aussi quand le jeu s'est fermé de lui-même : GameScope et Wine
    /// détachent leurs services de la session, ils ne meurent pas avec elle.
    pub fn reap_leftovers(&self) {
        if let Some(prefix) = self.compat_data.as_deref() {
            reap_prefix(prefix);
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Une session abandonnée (erreur après lancement, daemon en fin de
        // vie) laisserait sinon son processus zombie jusqu'à la mort du
        // daemon. On récolte ce qui est déjà mort, sans attendre ni tuer.
        let _ = self.child.try_wait();
    }
}

/// Représentation lisible de la commande préparée (dry-run, journaux).
pub fn render_prepared(prepared: &PreparedLaunch) -> String {
    let mut out = String::new();
    for (key, value) in prepared.command.get_envs() {
        if let Some(value) = value {
            out.push_str(&format!(
                "{}={}\n",
                key.to_string_lossy(),
                value.to_string_lossy()
            ));
        }
    }
    out.push_str(&prepared.command.get_program().to_string_lossy());
    for arg in prepared.command.get_args() {
        out.push(' ');
        out.push_str(&arg.to_string_lossy());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn une_session_arretee_est_recoltee() {
        // Sans récolte, GameScope tué reste zombie et `/proc/<pid>` demeure :
        // le pont Steamworks n'était alors jamais restauré.
        let dir =
            std::env::temp_dir().join(format!("lapinux-session-recolte-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut command = Command::new("sh");
        command.arg("-c").arg("sleep 30");
        let prepared = PreparedLaunch {
            command,
            log_path: dir.join("game.log"),
            gamescope: false,
            scene: false,
            scene_exit_path: None,
            scene_start_path: None,
            stats_path: None,
            gamescope_warning: None,
        };
        let mut session = Session::spawn(1, prepared).expect("session lancée");
        let pid = session.pid;
        let entry = format!("/proc/{pid}");
        assert!(std::path::Path::new(&entry).exists());

        session.terminate();
        assert!(
            !std::path::Path::new(&entry).exists(),
            "le processus de session est resté zombie ({entry})"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Résumé lisible des couches activées, pour l'UI et les journaux.
pub fn layers_summary(gamescope: bool) -> String {
    let mut parts = Vec::new();
    if gamescope {
        parts.push("GameScope");
    }
    if parts.is_empty() {
        "lancement direct".into()
    } else {
        parts.join(" + ")
    }
}
