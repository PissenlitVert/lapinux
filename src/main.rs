//! Lapinux — gestionnaire de jeux léger pour Linux.

mod cli;

use lapinux_core::paths::LapinuxPaths;

fn main() {
    // Avant tout fil : la mémoire du processus en dépend (voir plus bas).
    limit_allocator_arenas();
    init_logging();
    // Avant le premier échange TLS : la session sociale et les téléchargements
    // en dépendent, et rustls ne peut pas choisir seul son fournisseur ici.
    lapinux_steam::install_crypto_provider();
    // Lapinux se relance lui-même comme processus hôte, quel que soit le
    // binaire d'origine : la variable tranche avant l'analyse des arguments.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if std::env::var("LAPINUX_MODE").as_deref() == Ok("host") {
        std::process::exit(lapinux_steam::probe::cli(&args));
    }
    let code = match args.first().map(String::as_str) {
        Some("--daemon") => run_daemon(),
        Some("--version") | Some("-V") => {
            println!("{} {}", lapinux_core::APP_NAME, lapinux_core::VERSION);
            0
        }
        Some("--help") | Some("-h") | Some("help") => {
            cli::run(&["help".to_string()]);
            0
        }
        Some("overlay") => cli::overlay(&args[1..]),
        Some("scene") => run_scene(&args[1..]),
        Some("cli") => cli::run(&args[1..]),
        Some(other) if !other.starts_with('-') => {
            eprintln!("commande inconnue : {other}");
            cli::run(&["help".to_string()]);
            2
        }
        _ => run_gui(),
    };
    std::process::exit(code);
}

/// Borne le nombre d'arènes mémoire de glibc (`M_ARENA_MAX = -8`).
///
/// glibc réserve une arène par fil, jusqu'à soixante-quatre mégaoctets chacune.
/// Lapinux ouvre beaucoup de fils courts — jaquettes à télécharger, relevés
/// IPC, lancements — et la mémoire du processus montait à six ou huit cents
/// mégaoctets, puis au gigaoctet au défilement de la bibliothèque. Deux arènes
/// suffisent largement : l'empreinte mesurée devient stable à 268 Mo.
///
/// L'appel est déclaré ici plutôt que d'ajouter une dépendance : c'est une
/// fonction de la bibliothèque C du système, présente sur toutes les
/// distributions visées.
#[cfg(target_os = "linux")]
fn limit_allocator_arenas() {
    extern "C" {
        fn mallopt(param: std::os::raw::c_int, value: std::os::raw::c_int)
            -> std::os::raw::c_int;
    }
    const M_ARENA_MAX: std::os::raw::c_int = -8;
    // SAFETY: seul `M_ARENA_MAX` est réglé, avec une valeur positive, une fois
    // et avant que les fils ne se multiplient. L'échec est sans conséquence.
    unsafe {
        mallopt(M_ARENA_MAX, 2);
    }
}

#[cfg(not(target_os = "linux"))]
fn limit_allocator_arenas() {}

fn init_logging() {
    use tracing_subscriber::EnvFilter;
    let filter = lapinux_core::env_var("LOG")
        .and_then(|value| EnvFilter::try_new(value).ok())
        .or_else(|| EnvFilter::try_new("info").ok())
        .unwrap_or_default();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .try_init();
}

/// Scène persistante utilisée par le daemon autour du jeu dans GameScope.
fn run_scene(args: &[String]) -> i32 {
    let Some(separator) = args.iter().position(|arg| arg == "--") else {
        eprintln!(
            "usage : lapinux scene [--name NOM] [--width L] [--height H] [--backdrop IMAGE] [--cover IMAGE] [--end-secondes N] [--exit-file FICHIER] -- COMMANDE [ARGS...]"
        );
        return 2;
    };
    let mut name = lapinux_core::APP_NAME.to_string();
    let mut width = 1280_u32;
    let mut height = 720_u32;
    let mut cover = None;
    let mut backdrop = None;
    let mut end_seconds = 4.0_f32;
    let mut exit_file = None;
    let mut index = 0;
    while index < separator {
        match args[index].as_str() {
            "--name" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("scene : --name attend une valeur");
                    return 2;
                };
                name = value.clone();
                index += 2;
            }
            "--end-secondes" => {
                let Some(value) = args.get(index + 1).and_then(|v| v.parse::<f32>().ok()) else {
                    eprintln!("scene : --end-secondes attend un nombre");
                    return 2;
                };
                end_seconds = value;
                index += 2;
            }
            "--cover" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("scene : --cover attend un chemin");
                    return 2;
                };
                cover = Some(std::path::PathBuf::from(value));
                index += 2;
            }
            "--backdrop" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("scene : --backdrop attend un chemin");
                    return 2;
                };
                backdrop = Some(std::path::PathBuf::from(value));
                index += 2;
            }
            "--width" => {
                let Some(value) = args.get(index + 1).and_then(|v| v.parse::<u32>().ok()) else {
                    eprintln!("scene : --width attend un nombre");
                    return 2;
                };
                width = value.max(320);
                index += 2;
            }
            "--height" => {
                let Some(value) = args.get(index + 1).and_then(|v| v.parse::<u32>().ok()) else {
                    eprintln!("scene : --height attend un nombre");
                    return 2;
                };
                height = value.max(240);
                index += 2;
            }
            "--exit-file" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("scene : --exit-file attend un chemin");
                    return 2;
                };
                exit_file = Some(std::path::PathBuf::from(value));
                index += 2;
            }
            other => {
                eprintln!("scene : option inconnue « {other} »");
                return 2;
            }
        }
    }
    let command = args[separator + 1..].to_vec();
    if command.is_empty() {
        eprintln!("scene : aucune commande après --");
        return 2;
    }
    match lapinux_ui::run_scene(lapinux_ui::SceneOptions {
        name,
        width,
        height,
        cover,
        backdrop,
        command,
        end_seconds,
        exit_file,
    }) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("scène : {err}");
            1
        }
    }
}

fn run_daemon() -> i32 {
    let paths = LapinuxPaths::discover();
    match lapinux_daemon::run(paths, None) {
        Ok(()) => 0,
        Err(err) => {
            tracing::error!("daemon : {err}");
            eprintln!("daemon : {err}");
            1
        }
    }
}

/// Marqueur d'une session morte pendant la création de sa fenêtre.
///
/// Le pilote graphique de certaines machines fait tomber le contexte GL
/// (`libnvidia-glcore`). Le fichier est posé au lancement et retiré cinq
/// secondes plus tard : s'il survit, la session est morte en route et le
/// lancement suivant passe par le rendu logiciel.
fn wayland_marker() -> std::path::PathBuf {
    LapinuxPaths::discover().data_dir.join("gui-wayland-crashed")
}

/// Deuxième jeu de bibliothèques EGL/GLX, celui du pilote libre.
const MESA_EGL: &str = "/usr/share/glvnd/egl_vendor.d/50_mesa.json";

/// Après un plantage du pilote officiel, dessine avec Mesa sur le processeur.
/// Ces variables sont lues à l'initialisation d'EGL/GLX, c'est-à-dire avant la
/// fenêtre : les poser ici suffit à orienter la session.
fn force_soft_renderer() {
    std::env::set_var("__EGL_VENDOR_LIBRARY_FILENAMES", MESA_EGL);
    std::env::set_var("__GLX_VENDOR_LIBRARY_NAME", "mesa");
    std::env::set_var("LIBGL_ALWAYS_SOFTWARE", "1");
    std::env::set_var("GALLIUM_DRIVER", "llvmpipe");
}

fn run_gui() -> i32 {
    let marker = wayland_marker();
    let age = std::fs::metadata(&marker)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|when| when.elapsed().ok());
    match age {
        // Marqueur récent : la session précédente est morte ici.
        Some(elapsed) if elapsed.as_secs() < 7 * 24 * 3600 => {
            tracing::warn!(
                "interface : le pilote graphique a fait tomber la fenêtre ; rendu logiciel (Mesa)"
            );
            force_soft_renderer();
        }
        other => {
            if other.is_some() {
                // Marqueur très ancien : on redonne sa chance au pilote officiel.
                let _ = std::fs::remove_file(&marker);
            }
            if let Some(dir) = marker.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&marker, b"");
            let path = marker.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(5));
                let _ = std::fs::remove_file(&path);
            });
        }
    }
    match lapinux_ui::run() {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("interface : {err}");
            1
        }
    }
}
