//! Lapinux — hôte natif Steam (expérimental).
//!
//! `lapinux-probe` charge la bibliothèque du client Steam de Valve dans son
//! propre processus et rapporte jusqu'où ses interfaces répondent sans client
//! lancé. `--session` tente en plus une session locale, dans un processus
//! isolé : si le moteur tombe, seul l'enfant tombe. `account` interroge les
//! deux backends de compte : session locale du moteur et authentification par
//! le protocole CM, anonyme ou nominative.

use lapinux_steam::probe;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("--version") | Some("-V") => {
            println!(
                "{} {} — Lapinux, hôte natif expérimental",
                probe::COMMAND,
                env!("CARGO_PKG_VERSION")
            );
            0
        }
        Some("--help") | Some("-h") | Some("help") => {
            help();
            0
        }
        _ => probe::cli(&args),
    };
    std::process::exit(code);
}

fn help() {
    println!(
        "Lapinux — hôte natif autour de la bibliothèque du client Steam.\n\
         \n\
         Utilisation :\n\
         \x20 lapinux-probe               sonde : bibliothèque, interfaces, pipe\n\
         \x20 lapinux-probe --session     tente une session locale (processus isolé)\n\
         \x20 lapinux-probe probe         forme explicite de la sonde\n\
         \x20 lapinux-probe account       compte Steam : session locale + CM\n\
         \x20   --login COMPTE              connexion nominative (jeton réutilisé,\n\
         \x20                               sinon mot de passe et Steam Guard)\n\
         \x20   --local                     ne teste que la session locale du moteur\n\
         \x20   --cm                        ne teste que le protocole CM\n\
         \x20   --endpoint HOST:PORT        CM imposé (sinon découverte WebAPI)\n\
         \x20 lapinux-probe --version     version\n\
         \n\
         Le parcours historique mesure le moteur déjà installé et ouvre une session\n\
         CM anonyme ou nominative. Le mot de passe n'est jamais passé en argument\n\
         ni conservé ; le jeton est écrit en 0600 dans le dossier de données.\n\
         Le processus hôte apparaît sous le nom « {} ».\n\
         \n\
         Variables :\n\
         \x20 LAPINUX_STEAMCLIENT         force le chemin de steamclient.so (historique)\n\
         \x20 LAPINUX_STEAM_CM           liste de CMs host:port (sinon WebAPI)\n\
         \x20 (les anciennes variables LAPINUX_STEAM* restent lues)",
        probe::HOST_PROCESS
    );
}
