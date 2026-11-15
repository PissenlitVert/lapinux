//! Sonde des onglets Steam Input et Atelier : exerce les requêtes IPC réelles.
//!
//! ```bash
//! HOME=/tmp/lapinux-e2e target/release/lapinux --daemon &
//! HOME=/tmp/lapinux-e2e cargo run -p lapinux-daemon --example steam_tabs_probe -- 443510 "Nom" --arrange
//! ```

use lapinux_core::client::Client;
use lapinux_core::ipc::Req;
use lapinux_core::model::{SteamInputReport, WorkshopArrangeReport, WorkshopReport};
use lapinux_core::paths::LapinuxPaths;

fn request<T: serde::de::DeserializeOwned>(client: &Client, req: &Req) -> Result<T, String> {
    client
        .request(req)
        .map_err(|error| error.to_string())?
        .into_data()
        .map_err(|error| error.to_string())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("sonde : {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let appid: u32 = args
        .first()
        .and_then(|value| value.parse().ok())
        .unwrap_or(443510);
    let name = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| format!("Jeu {appid}"));
    let arrange = args.iter().any(|arg| arg == "--arrange");
    let shutdown = args.iter().any(|arg| arg == "--shutdown");
    let paths = LapinuxPaths::discover();
    let client = Client::new(&paths).without_spawn();
    println!("socket : {}", paths.socket.display());
    match client.daemon_build() {
        Some(build) => println!("daemon : {} (stamp {})", build.version, build.stamp),
        None => return Err(format!("aucun daemon sur {}", paths.socket.display())),
    }

    let input: SteamInputReport = request(&client, &Req::SteamInput { appid })?;
    println!(
        "\nSteam Input — {} manette(s), {} profil(s), disponible={}",
        input.controllers.len(),
        input.profiles.len(),
        input.available
    );
    for controller in &input.controllers {
        println!(
            "  manette : {} · {} · {:04x}:{:04x}",
            controller.name, controller.label, controller.vendor, controller.product
        );
    }
    let offered = input.for_connected();
    println!("  profils pour les manettes présentes : {}", offered.len());
    for profile in offered.iter().take(12) {
        println!(
            "  · [{}] {} (appid {}, {})",
            profile.controller_type, profile.title, profile.appid, profile.origin
        );
    }
    for note in &input.notes {
        println!("  note : {note}");
    }

    let report: WorkshopReport = request(
        &client,
        &Req::SteamWorkshop {
            appid,
            name: name.clone(),
            refresh: false,
        },
    )?;
    println!(
        "\nAtelier — {} abonnement(s), total {}, installé {}, liens {}, support={}",
        report.items.len(),
        lapinux_core::util::format_bytes(report.total_size),
        lapinux_core::util::format_bytes(report.installed_size),
        report.links,
        report.supported
    );
    println!("  dossier : {}", report.mods_dir.display());
    println!("  page : {}", report.workshop_url);
    for item in report.items.iter().take(8) {
        println!(
            "  · {} | {} | {} | installé={}",
            item.published_file_id,
            if item.title.trim().is_empty() {
                "(sans titre)"
            } else {
                item.title.as_str()
            },
            lapinux_core::util::format_bytes(item.size),
            item.installed
        );
    }
    for note in &report.notes {
        println!("  note : {note}");
    }

    if arrange {
        println!("\nRangement 1…");
        let first: WorkshopArrangeReport = request(
            &client,
            &Req::SteamWorkshopArrange {
                appid,
                name: name.clone(),
            },
        )?;
        println!(
            "  {} lien(s), {} retiré(s), {} dans {}",
            first.links,
            first.removed,
            lapinux_core::util::format_bytes(first.bytes),
            first.dir.display()
        );
        println!("Rangement 2…");
        let second: WorkshopArrangeReport = request(
            &client,
            &Req::SteamWorkshopArrange {
                appid,
                name: name.clone(),
            },
        )?;
        println!(
            "  {} lien(s), {} retiré(s), {} dans {}",
            second.links,
            second.removed,
            lapinux_core::util::format_bytes(second.bytes),
            second.dir.display()
        );
        let after: WorkshopReport = request(
            &client,
            &Req::SteamWorkshop {
                appid,
                name,
                refresh: false,
            },
        )?;
        println!(
            "  après rangement : {} lien(s) vus par le rapport",
            after.links
        );
        let entries = std::fs::read_dir(&second.dir)
            .map_err(|error| format!("lecture de {} : {error}", second.dir.display()))?
            .flatten()
            .count();
        println!("  entrées dans le dossier : {entries}");
    }

    if shutdown {
        let _ = client.request(&Req::Shutdown);
    }
    Ok(())
}
