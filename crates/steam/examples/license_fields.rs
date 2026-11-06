//! Diagnostic : les champs que le CM publie vraiment dans une entrée de
//! licence — paquet, date d'ajout et moyen de paiement.
//!
//! ```text
//! cargo run -p lapinux-steam --example license_fields
//! ```

use lapinux_steam::cm;
use lapinux_steam::session;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();
    let saved = session::load().expect("aucune session Steam enregistrée");
    let config = cm::Config::from_env();
    let auth = cm::Auth::Token {
        name: saved.account_name.clone(),
        refresh_token: saved.refresh_token.clone(),
    };
    let mut prompt = |_, _| Err(cm::Error::Auth("aucun code Steam Guard attendu".into()));
    let session = cm::sign_in(&config, &auth, &mut prompt).expect("session CM");
    let licenses = session.licenses.unwrap_or_default();
    println!("{} licences reçues", licenses.len());
    println!(
        "datées : {}",
        licenses.iter().filter(|licence| licence.time_created > 0).count()
    );
    println!(
        "avec moyen de paiement : {}",
        licenses
            .iter()
            .filter(|licence| licence.payment_method > 0)
            .count()
    );
    for licence in licenses
        .iter()
        .filter(|licence| licence.owner_id == 0)
        .take(10)
    {
        println!(
            "paquet {} · créée {} · paiement {}",
            licence.package_id, licence.time_created, licence.payment_method
        );
    }
}
