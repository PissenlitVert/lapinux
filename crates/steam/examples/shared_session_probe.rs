//! Diagnostic : toutes les lectures doivent partager **une** session Steam.
//!
//! Deux fiches lues de suite coûtent un logon, pas deux. Lançer avec
//! `RUST_LOG=lapinux_steam=debug` pour voir « session Steam ouverte » une seule
//! fois, puis « session Steam réutilisée ».

use lapinux_steam::gamedata::{self, Sources};
use lapinux_steam::session;

fn main() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(false)
        .try_init();
    let saved = session::load().expect("aucune session Steam enregistrée");
    let sources = Sources {
        account: saved.account_name.clone(),
        refresh_token: saved.refresh_token.clone(),
        steam_id: saved.steam_id,
    };
    let appids: Vec<u32> = std::env::args()
        .skip(1)
        .filter_map(|arg| arg.parse().ok())
        .collect();
    let appids = if appids.is_empty() {
        vec![3764200, 1245620]
    } else {
        appids
    };
    for appid in appids {
        let started = std::time::Instant::now();
        let info = gamedata::game_info(appid, &sources, None);
        println!(
            "{appid} · {} ms · sources {:?} · note {:?}",
            started.elapsed().as_millis(),
            info.sources,
            info.notes
        );
        println!(
            "    temps de jeu {:?} · niveau {:?} · badge {:?} · joueurs {:?}",
            info.account.minutes_total,
            info.account.level,
            info.account.badge_level,
            info.account.players_now
        );
        println!("    {} contenu(s) additionnel(s) :", info.update.dlc.len());
        for content in &info.update.dlc {
            println!(
                "      {} · {} · {} · possédé {} · image «{}»",
                content.appid,
                content.name,
                lapinux_core::util::format_bytes(content.download),
                content.owned,
                content.image
            );
        }
    }
}
