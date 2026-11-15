//! Mesure : combien de temps le daemon met-il à publier l'URL du QR après la
//! demande ? La connexion est annulée à la fin.

use lapinux_core::client::Client;
use lapinux_core::ipc::Req;
use lapinux_core::model::SteamStatus;
use lapinux_core::paths::LapinuxPaths;
use std::time::{Duration, Instant};

fn main() {
    let paths = LapinuxPaths::discover();
    let client = Client::new(&paths).without_spawn();
    let started = Instant::now();
    match client.request(&Req::SteamLoginQr) {
        Ok(envelope) if envelope.ok => {}
        Ok(envelope) => {
            println!("refusé : {:?}", envelope.error);
            return;
        }
        Err(error) => {
            println!("daemon injoignable : {error}");
            return;
        }
    }
    let mut seen = None;
    while started.elapsed() < Duration::from_secs(30) {
        if let Ok(envelope) = client.request(&Req::SteamStatus) {
            if let Ok(status) = envelope.into_data::<SteamStatus>() {
                if status.login.qr.is_some() {
                    seen = Some(started.elapsed());
                    break;
                }
                if let Some(error) = status.login.error {
                    println!("erreur après {:?} : {error}", started.elapsed());
                    return;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    match seen {
        Some(elapsed) => println!("QR publié après {:.0} ms", elapsed.as_secs_f64() * 1000.0),
        None => println!("aucun QR en 30 s"),
    }
    let _ = client.request(&Req::SteamLoginCancel);
}
