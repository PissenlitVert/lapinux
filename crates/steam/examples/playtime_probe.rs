//! Sonde du temps de jeu lu par le protocole pour un ou plusieurs appids.
//!
//! Sert à savoir ce que Steam publie vraiment pour un jeu : `GetOwnedGames`
//! répond-il pour un jeu prêté par la famille, avec quel temps total ?
//!
//! ```text
//! cargo run -p lapinux-steam --example playtime_probe -- 3764200 620
//! ```

use lapinux_steam::{native, playtime, session};

fn main() {
    let appids: Vec<u32> = std::env::args()
        .skip(1)
        .filter_map(|value| value.parse().ok())
        .collect();
    let appids = if appids.is_empty() {
        vec![3764200]
    } else {
        appids
    };
    let paths = lapinux_core::paths::LapinuxPaths::discover();
    let saved = match session::load_from(&session::path_in(&paths.data_dir)) {
        Some(saved) => saved,
        None => {
            println!("aucune session enregistrée");
            return;
        }
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("moteur réseau");
    runtime.block_on(async move {
        let client = match native::login(&saved.account_name, &saved.refresh_token).await {
            Ok(client) => client,
            Err(error) => {
                println!("connexion refusée : {error}");
                return;
            }
        };
        for appid in appids {
            let group = playtime::family_group_id(&client, saved.steam_id).await;
            match playtime::game_playtime(&client, appid, saved.steam_id, group).await {
                Ok(Some(play)) => println!(
                    "{appid} : {} min (total), {} min (2 semaines), {} min (linux), dernière partie {:?}, famille {}",
                    play.minutes_total,
                    play.minutes_2weeks,
                    play.minutes_linux,
                    play.last_played,
                    play.family_shared
                ),
                Ok(None) => println!(
                    "{appid} : le protocole ne publie aucun temps de jeu (groupe familial {group:?})"
                ),
                Err(error) => println!("{appid} : erreur de lecture ({error})"),
            }
        }
    });
}
