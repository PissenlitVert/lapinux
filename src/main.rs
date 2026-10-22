//! Ludus — gestionnaire de jeux léger pour Linux.

mod cli;

use ludus_core::paths::LudusPaths;

fn main() {
    init_logging();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("--daemon") => run_daemon(),
        Some("--version") | Some("-V") => {
            println!("ludus {} ({})", ludus_core::VERSION, env!("CARGO_PKG_VERSION"));
            0
        }
        Some("--help") | Some("-h") | Some("help") => {
            cli::run(&["help".to_string()]);
            0
        }
        Some("overlay") => cli::overlay(&args[1..]),
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

fn init_logging() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_env("LUDUS_LOG")
        .or_else(|_| EnvFilter::try_new("info"))
        .unwrap_or_default();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .try_init();
}

fn run_daemon() -> i32 {
    let paths = LudusPaths::discover();
    match ludus_daemon::run(paths, None) {
        Ok(()) => 0,
        Err(err) => {
            tracing::error!("daemon : {err}");
            eprintln!("daemon : {err}");
            1
        }
    }
}

fn run_gui() -> i32 {
    match ludus_ui::run() {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("interface : {err}");
            1
        }
    }
}
