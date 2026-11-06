//! Diagnostic : la deuxième lecture d'un jeu doit sortir du cache.
//!
//! Chaque lecture non cachée ouvre une session Steam ; Steam n'en garde qu'une
//! par compte. Deux appels au même jeu doivent donc coûter une session, pas
//! deux.

use std::time::Instant;

use lapinux_core::client::Client;
use lapinux_core::ipc::Req;
use lapinux_core::paths::LapinuxPaths;

fn main() {
    let appid: u32 = std::env::args()
        .nth(1)
        .and_then(|arg| arg.parse().ok())
        .unwrap_or(3764200);
    let paths = LapinuxPaths::discover();
    let client = Client::new(&paths);
    for pass in 1..=2 {
        let started = Instant::now();
        match client.request(&Req::GameAchievements { appid }) {
            Ok(envelope) => println!(
                "lecture {pass} : {} ms (ok = {})",
                started.elapsed().as_millis(),
                envelope.ok
            ),
            Err(error) => println!("lecture {pass} : échec ({error})"),
        }
    }
}
