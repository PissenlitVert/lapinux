//! Sonde jetable : la famille et le nombre de jeux de chaque membre.
use lapinux_core::social::SteamPresence;
use lapinux_steam::session;
use lapinux_steam::social::Social;
use std::time::{Duration, Instant};

fn main() {
    let saved = session::load().expect("aucune session Steam enregistrée");
    let social = Social::start(saved, SteamPresence::Online).expect("session sociale");
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut done = false;
    while Instant::now() < deadline && !done {
        std::thread::sleep(Duration::from_secs(2));
        let snapshot = social.snapshot();
        if let Some(family) = snapshot.family.as_ref() {
            println!("famille {:?} groupe {}", family.name, family.group_id);
            for member in &family.members {
                println!("  {} {} jeux={}", member.steam_id, member.name, member.games);
            }
            done = family.members.iter().any(|member| member.games > 0);
        }
    }
    let last = social.snapshot();
    println!("état {:?} erreur {:?} terminé={}", last.state, last.error, done);
}
