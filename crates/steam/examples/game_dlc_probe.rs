//! Diagnostic : la fiche d'un jeu doit nommer ses contenus additionnels, dire
//! lesquels sont au compte et combien ils pèsent.

use lapinux_core::client::Client;
use lapinux_core::ipc::Req;
use lapinux_core::model::SteamGameInfo;
use lapinux_core::paths::LapinuxPaths;

fn main() {
    let appid: u32 = std::env::args()
        .nth(1)
        .and_then(|arg| arg.parse().ok())
        .unwrap_or(3764200);
    let paths = LapinuxPaths::discover();
    let client = Client::new(&paths);
    let envelope = client
        .request(&Req::GameInfo { appid })
        .expect("le daemon n'a pas répondu");
    let info = envelope
        .into_data::<SteamGameInfo>()
        .expect("fiche illisible");
    let owned = info.update.dlc.iter().filter(|dlc| dlc.owned).count();
    println!("{appid} · {} · {owned} sur {} possédés", info.name, info.update.dlc.len());
    for content in &info.update.dlc {
        println!(
            "  {} · {} · {} · possédé {} · {}",
            content.appid,
            content.name,
            lapinux_core::util::format_bytes(content.download),
            content.owned,
            content.image
        );
    }
}
