//! Mesure le temps des lectures d'une fiche de boutique (résumé, succès,
//! cloud) par l'IPC du daemon.
//!
//! Sans argument, liste les jeux de boutique connus ; avec un identifiant,
//! chronomètre chaque lecture deux fois : à froid, puis depuis le cache.
//! `cargo run --release --example store_sheet_probe -- 12345`.

use lapinux_core::client::Client;
use lapinux_core::ipc::Req;
use lapinux_core::paths::LapinuxPaths;
use lapinux_core::stores::StoresView;
use std::time::Instant;

fn main() {
    let paths = LapinuxPaths::discover();
    let client = Client::new(&paths);
    let Some(appid) = std::env::args()
        .nth(1)
        .and_then(|value| value.parse::<u32>().ok())
    else {
        let view = client
            .request(&Req::Stores { refresh: false, only: Vec::new() })
            .and_then(|envelope| envelope.into_data::<StoresView>())
            .expect("vue des boutiques");
        for library in &view.libraries {
            for game in library.entries.iter().take(8) {
                println!(
                    "{:>10}  {:<10}  {}",
                    game.appid,
                    game.store.label(),
                    game.name
                );
            }
        }
        return;
    };
    for attempt in 1..=2 {
        println!("— passage {attempt} —");
        for (name, request) in [
            ("résumé", Req::StoreDetails { appid }),
            ("succès", Req::GameAchievements { appid }),
            ("cloud", Req::SteamCloud { appid }),
        ] {
            let started = Instant::now();
            let reply = client.request(&request);
            let elapsed = started.elapsed().as_millis();
            match reply {
                Ok(envelope) => {
                    let bytes = serde_json::to_string(&envelope.data)
                        .map(|text| text.len())
                        .unwrap_or(0);
                    println!(
                        "{name:<8} {elapsed:>6} ms · ok={} · {bytes} octets",
                        envelope.ok
                    );
                }
                Err(error) => println!("{name:<8} {elapsed:>6} ms · erreur {error}"),
            }
        }
    }
}
