//! Mesure le temps de réponse d'une fiche de jeu par l'IPC du daemon.
//!
//! Sert à vérifier que la deuxième ouverture d'un jeu ne repaie pas le
//! protocole : `cargo run --release --example info_probe -- 1091500`.

use lapinux_core::client::Client;
use lapinux_core::ipc::Req;
use lapinux_core::paths::LapinuxPaths;
use std::time::Instant;

fn main() {
    let paths = LapinuxPaths::discover();
    let client = Client::new(&paths);
    let appid: u32 = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(1_091_500);
    let tries: usize = std::env::args()
        .nth(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(3);
    for attempt in 1..=tries {
        let started = Instant::now();
        let reply = client.request(&Req::GameInfo { appid });
        let elapsed = started.elapsed().as_millis();
        match reply {
            Ok(envelope) => {
                let bytes = serde_json::to_string(&envelope.data)
                    .map(|text| text.len())
                    .unwrap_or(0);
                println!(
                    "essai {attempt} : {elapsed} ms · ok={} · {bytes} octets",
                    envelope.ok
                );
            }
            Err(error) => println!("essai {attempt} : {elapsed} ms · erreur {error}"),
        }
    }
}
