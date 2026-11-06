//! Sonde : l'envoi d'une sauvegarde vers le cloud, de bout en bout.
//!
//! Elle dépose un petit fichier de test dans le dossier cloud du jeu, puis le
//! supprime : rien de la sauvegarde du compte n'est touché.
//!
//! ```text
//! cargo run -p lapinux-steam --example cloud_upload_probe --features probes -- 2830030
//! ```

use lapinux_core::model::CloudFile;
use lapinux_steam::cm::proto::{field_bytes, field_varint};
use lapinux_steam::{cloud, native, session};

fn main() {
    let appid: u32 = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(2830030);
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
        let state = match cloud::game_cloud(&client, appid).await {
            Ok(state) => state,
            Err(error) => {
                println!("liste cloud illisible : {error}");
                return;
            }
        };
        let Some(prefix) = state.prefixes.first().cloned() else {
            println!("aucun dossier cloud déclaré");
            return;
        };
        let name = format!("{prefix}lapinux-probe.txt");
        let data = b"lapinux : fichier de test, supprime aussitot.\n";
        let file = CloudFile {
            name: "lapinux-probe.txt".into(),
            size: data.len() as u64,
            prefix: prefix.clone(),
            ..Default::default()
        };
        println!("dossier : {prefix} (changement {})", state.change_number);
        let batch = match cloud::begin_batch(&client, appid, "lapinux-probe", &[name.clone()]).await {
            Ok(batch) => batch,
            Err(error) => {
                println!("lot refusé : {error}");
                return;
            }
        };
        println!("lot ouvert : {batch}");
        match cloud::upload_file(&client, appid, &file, data, batch).await {
            Ok(()) => println!("fichier envoyé et validé"),
            Err(error) => println!("envoi refusé : {error}"),
        }
        match cloud::complete_batch(&client, appid, batch).await {
            Ok(()) => println!("lot clos"),
            Err(error) => println!("clôture refusée : {error}"),
        }
        // Suppression du fichier de test : le cloud du compte retrouve son état.
        // C'est la variante serveur qui l'accepte, la variante « client »
        // n'étant valable que pour un lot en cours.
        let mut body = Vec::new();
        field_bytes(1, name.as_bytes(), &mut body);
        field_varint(2, u64::from(appid), &mut body);
        match client.call_service_method("Cloud.Delete#1", &body).await {
            Ok(_) => println!("fichier de test supprimé"),
            Err(error) => println!("suppression impossible : {error}"),
        }
        println!("hors ligne");
    });
}
