//! Sonde : la session sociale telle qu'elle sera vécue par l'application.
//!
//! Elle se connecte au CM avec le jeton de refresh enregistré, publie la
//! présence demandée, attend que Steam pousse la liste d'amis, les états, la
//! famille et les notifications, puis affiche l'instantané. Comme
//! l'application, elle entretient le jeton au passage : lancez-la avec
//! `LAPINUX_SOCIAL_NO_RENEW=1` pour observer sans y toucher.
//!
//! ```text
//! LAPINUX_SOCIAL_NO_RENEW=1 cargo run -p lapinux-steam --example social_probe -- \
//!     [en ligne|absent|occupe|invisible|hors ligne] [secondes]
//! ```

use lapinux_core::social::SteamPresence;
use lapinux_steam::session;
use lapinux_steam::social::Social;
use std::time::{Duration, Instant};

fn presence_from_argument(value: &str) -> SteamPresence {
    match value.to_lowercase().as_str() {
        "absent" | "away" => SteamPresence::Away,
        "occupe" | "occupé" | "busy" => SteamPresence::Busy,
        "invisible" => SteamPresence::Invisible,
        "hors ligne" | "hors-ligne" | "offline" => SteamPresence::Offline,
        _ => SteamPresence::Online,
    }
}

fn main() {
    // `RUST_LOG=lapinux_steam=debug` montre la conversation avec le CM.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    let mut arguments = std::env::args().skip(1);
    let presence = arguments
        .next()
        .map(|value| presence_from_argument(&value))
        .unwrap_or(SteamPresence::Online);
    let seconds: u64 = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(20);

    let saved = session::load().expect("aucune session Steam enregistrée");
    println!(
        "=== session : {} (steamid {}), présence demandée : {}",
        saved.account_name,
        saved.steam_id,
        presence.label()
    );
    let social = Social::start(saved, presence).expect("session sociale");

    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut last_state = String::new();
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_secs(2));
        let snapshot = social.snapshot();
        if snapshot.state != last_state {
            println!(
                "--- état {} (erreur {:?}), présence appliquée {}",
                snapshot.state,
                snapshot.error,
                snapshot.applied_presence.label()
            );
            last_state = snapshot.state.clone();
        }
        if snapshot.live() {
            // Le temps que les amis arrivent après le logon.
            if snapshot.friends.len() > 0 && Instant::now() + Duration::from_secs(4) > deadline {
                break;
            }
        }
    }

    let snapshot = social.snapshot();
    println!(
        "=== état : {} · présence {} / appliquée {}",
        snapshot.state,
        snapshot.presence.label(),
        snapshot.applied_presence.label()
    );
    println!(
        "=== profil : {} (steamid {})",
        snapshot.persona_name, snapshot.steam_id
    );
    println!(
        "=== amis : {} en ligne, {} au total",
        snapshot.online_friends().len(),
        snapshot.friends.len()
    );
    for friend in snapshot.online_friends().iter().take(8) {
        println!(
            "    {} — {} (dernière connexion {}, type {}, id {})",
            friend.name,
            friend.state_line(),
            friend.last_logon,
            (friend.steam_id >> 52) & 0xf,
            friend.steam_id
        );
    }
    for friend in snapshot.offline_friends().iter().take(4) {
        println!("    {} — hors ligne", friend.name);
    }
    match &snapshot.family {
        Some(family) => {
            println!(
                "=== famille : {} ({} membres)",
                family.name,
                family.members.len()
            );
            for member in family.members.iter().take(8) {
                println!(
                    "    {} — {:?} rôle {} photo {} {}",
                    member.steam_id,
                    member.name,
                    member.role,
                    if member.avatar.is_empty() {
                        "non"
                    } else {
                        "oui"
                    },
                    if member.is_self { "(vous)" } else { "" }
                );
            }
        }
        None => println!("=== famille : aucune"),
    }
    println!("=== notifications : {}", snapshot.badge_count());
    for notification in &snapshot.notifications {
        println!("    {} × {}", notification.count, notification.label());
    }
    if snapshot.new_comments > 0 {
        println!("    {} nouveaux commentaires", snapshot.new_comments);
    }
    social.stop();
}
