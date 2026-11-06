//! Sonde de la page des licences : obtient-on une session web, et la page
//! porte-t-elle des dates d'achat ?
//!
//! Pour une session de client, le badge du site est le jeton d'accès du CM
//! (`<steamid>||<jeton>`). La page des licences, elle, ne porte aucun appid :
//! la sonde vérifie seulement que les lignes se lisent et que la date y est.
//! Rien n'est écrit — le catalogue n'est pas touché — et seuls des comptages
//! sortent, jamais le contenu du compte.

use lapinux_steam::{session, web};

fn main() {
    let paths = lapinux_core::paths::LapinuxPaths::discover();
    let saved = session::load_from(&session::path_in(&paths.data_dir))
        .filter(|saved| !saved.refresh_token.trim().is_empty());
    let Some(saved) = saved else {
        eprintln!("aucune session Steam enregistrée : connectez le compte d'abord");
        std::process::exit(2);
    };
    println!("compte : {}", saved.account_name);

    let token = match web::access_token(&saved.account_name, &saved.refresh_token, saved.steam_id) {
        Ok(token) => {
            println!("jeton d'accès du CM : reçu ({} caractères)", token.len());
            token
        }
        Err(error) => {
            eprintln!("jeton d'accès refusé : {error}");
            std::process::exit(1);
        }
    };

    let cookies = web::cookies_from_access_token(saved.steam_id, &token);
    let page = match web::licenses(&cookies) {
        Ok(page) => page,
        Err(error) => {
            eprintln!("page des licences : {error}");
            std::process::exit(1);
        }
    };
    println!(
        "page licences : statut {}, {} octets",
        page.status,
        page.body.len()
    );
    if page.status != 200 || page.body.contains("login") && page.body.len() < 20_000 {
        eprintln!("la page n'est pas celle du compte (session refusée ?)");
        std::process::exit(3);
    }

    let rows = web::parse_licenses(&page.body);
    let dated = rows.iter().filter(|row| row.acquired.is_some()).count();
    println!("licences lues : {} ligne(s), {dated} datée(s)", rows.len());

    let insurgency: Vec<&web::LicenseRow> = rows
        .iter()
        .filter(|row| row.name.to_lowercase().contains("insurgency"))
        .collect();
    println!(
        "Insurgency : {} ligne(s), {} datée(s)",
        insurgency.len(),
        insurgency
            .iter()
            .filter(|row| row.acquired.is_some())
            .count()
    );
    for row in &insurgency {
        println!(
            "  {} → {}",
            row.name,
            match row.acquired {
                Some(stamp) => format!("date {stamp}"),
                None => "sans date".to_string(),
            }
        );
    }

    // Les premières lignes datées, sans leur nom : seulement la date et le type
    // de licence, pour juger le parseur.
    for row in rows.iter().filter(|row| row.acquired.is_some()).take(3) {
        println!(
            "  ligne datée : type «{}» → {}",
            row.kind,
            row.acquired.unwrap_or_default()
        );
    }
}
