//! Diagnostic : quels dépôts le compte a le droit d'installer.
//!
//! Le protocole ne rend la clé de déchiffrement d'un dépôt qu'aux licences qui
//! le couvrent. La sonde demande la clé de chaque dépôt candidat et dit, pour
//! chacun, si Steam l'autorise : de quoi distinguer les dépôts d'une édition
//! que le compte ne possède pas.

use lapinux_steam::session;
use steamroom::depot::{AppId, DepotId};
use steamroom_client::login::LoginBuilder;

#[tokio::main]
async fn main() {
    let pairs: Vec<(u32, u32)> = std::env::args()
        .skip(1)
        .filter_map(|arg| {
            let (app, depot) = arg.split_once('/')?;
            Some((app.parse().ok()?, depot.parse().ok()?))
        })
        .collect();
    let saved = session::load().expect("aucune session Steam enregistrée");
    let client = LoginBuilder::new()
        .device_name("Lapinux (clés de dépôt)")
        .with_refresh_token(&saved.account_name, &saved.refresh_token)
        .login()
        .await
        .unwrap();
    for (app, depot) in pairs {
        match client
            .get_depot_decryption_key(DepotId(depot), AppId(app))
            .await
        {
            Ok(_) => println!("{app}/{depot} : autorisé"),
            Err(error) => println!("{app}/{depot} : refusé ({error})"),
        }
    }
}
