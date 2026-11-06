//! Sonde : sauvegardes Steam Cloud publiées pour un ou plusieurs appids.
//!
//! Sert à confirmer ce que le service `Cloud` répond pour le compte : liste
//! des fichiers, dates, dossiers déclarés et quota consommé.
//!
//! ```text
//! cargo run -p lapinux-steam --example cloud_probe --features probes -- 3764200
//! ```

use lapinux_steam::{cloud, native, session};

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
            match cloud::game_cloud(&client, appid).await {
                Ok(state) => {
                    println!(
                        "{appid} : {} fichier(s), {} o, changement {}, quota {}/{} fichiers · {}/{} o",
                        state.files.len(),
                        state.total_bytes(),
                        state.change_number,
                        state.used_files.unwrap_or(0),
                        state.max_files.unwrap_or(0),
                        state.used_bytes.unwrap_or(0),
                        state.max_bytes.unwrap_or(0)
                    );
                    for prefix in &state.prefixes {
                        println!("  dossier déclaré : {prefix}");
                    }
                    for file in &state.files {
                        println!(
                            "  {} · {} o · {}",
                            file.name,
                            file.size,
                            lapinux_core::util::format_datetime(file.modified)
                        );
                    }
                }
                Err(error) => println!("{appid} : erreur de lecture ({error})"),
            }
        }
    });
}
