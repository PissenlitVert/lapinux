//! Sonde : comment récupérer les octets d'une sauvegarde cloud.
//!
//! Trois pistes sont essayées pour un même fichier : `ClientFileDownload` avec
//! le nom seul, avec le dossier déclaré collé devant, puis `EnumerateUserFiles`
//! qui rend une adresse directe.
//!
//! ```text
//! cargo run -p lapinux-steam --example cloud_download_probe --features probes -- 2830030
//! ```

use lapinux_steam::cm::proto::{field_varint, Field, Reader};
use lapinux_steam::{cloud, native, session};
use steamroom::client::LoggedIn;
use steamroom::client::SteamClient;

fn text(field: Field<'_>) -> String {
    match field {
        Field::Bytes(bytes) => String::from_utf8_lossy(bytes).into_owned(),
        _ => String::new(),
    }
}

/// `Cloud.EnumerateUserFiles` : les fichiers du compte avec leur adresse.
async fn enumerate(client: &SteamClient<LoggedIn>, appid: u32) -> Vec<(String, String)> {
    let mut body = Vec::new();
    field_varint(1, u64::from(appid), &mut body);
    field_varint(2, 1, &mut body); // détails étendus : l'URL en fait partie
    field_varint(3, 50, &mut body);
    field_varint(4, 0, &mut body);
    let answer = match client
        .call_service_method("Cloud.EnumerateUserFiles#1", &body)
        .await
    {
        Ok(answer) => answer,
        Err(error) => {
            println!("EnumerateUserFiles refusé : {error}");
            return Vec::new();
        }
    };
    let mut files = Vec::new();
    let mut reader = Reader::new(&answer.body);
    while let Some((number, field)) = reader.next() {
        if number != 1 {
            continue;
        }
        let Field::Bytes(entry) = field else { continue };
        let mut name = String::new();
        let mut url = String::new();
        let mut inner = Reader::new(entry);
        while let Some((field, value)) = inner.next() {
            match field {
                3 => name = text(value),
                6 => url = text(value),
                _ => {}
            }
        }
        files.push((name, url));
    }
    files
}

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
        for file in &state.files {
            println!("--- {} ({} octets)", file.name, file.size);
            match cloud::download_file(&client, appid, file).await {
                Ok(bytes) => {
                    println!("  téléchargé : {} octets", bytes.len());
                    let path = format!("/tmp/cloud_payload_{}", file.name);
                    let _ = std::fs::write(&path, &bytes);
                    println!("    gardé dans {path}");
                }
                Err(error) => println!("  échec : {error}"),
            }
        }
        println!("--- EnumerateUserFiles");
        for (name, url) in enumerate(&client, appid).await {
            println!("  {name} : {url}");
        }
    });
}
