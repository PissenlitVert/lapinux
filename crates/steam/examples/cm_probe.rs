//! Sonde de connexion QR : affiche chaque défi publié par Steam pendant un peu
//! plus de deux minutes, puis annule (aucun téléphone n'approuve ici). Elle
//! vérifie que le QR reste vivant au lieu d'échouer au bout de trois minutes.

use lapinux_steam::cm;
use std::time::Instant;

fn main() {
    let config = cm::Config::from_env();
    let started = Instant::now();
    let mut seen: Vec<String> = Vec::new();
    let mut on_url = |url: &str| {
        println!(
            "défi {} à {:.1} s : {}",
            seen.len() + 1,
            started.elapsed().as_secs_f32(),
            url
        );
        seen.push(url.to_string());
    };
    let cancelled = || started.elapsed().as_secs() >= 130;
    match cm::sign_in_qr(&config, &mut on_url, &cancelled) {
        Ok(session) => println!("connexion acceptée : steamid {}", session.steam_id),
        Err(error) => println!("fin : {error}"),
    }
    let distinct = seen.iter().collect::<std::collections::HashSet<_>>().len();
    println!("{} défi(s) publié(s), {distinct} URL(s) distincte(s)", seen.len());
}
