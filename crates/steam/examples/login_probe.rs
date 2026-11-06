//! Diagnostic de connexion : CM joignable, puis jeton enregistré.
//!
//! Un logon **anonyme** ne touche à aucun compte : s'il aboutit, le réseau et
//! les CM vont bien et le refus vient du compte lui-même. Le logon par jeton
//! qui suit dit lequel des deux échoue, et avec quel code Steam.

use lapinux_steam::cm;
use lapinux_steam::session;

fn main() {
    lapinux_steam::install_crypto_provider();
    let config = cm::Config::from_env();
    println!("--- logon anonyme (aucun compte touché)");
    match cm::sign_in_anonymous(&config) {
        Ok(session) => println!(
            "anonyme : ok · cell {:?} · pays {:?}",
            session.cell_id, session.country
        ),
        Err(error) => println!("anonyme : échec ({error})"),
    }
    println!("--- logon par le jeton enregistré");
    let Some(saved) = session::load() else {
        println!("aucun jeton enregistré");
        return;
    };
    let auth = cm::Auth::Token {
        name: saved.account_name.clone(),
        refresh_token: saved.refresh_token.clone(),
    };
    let mut prompt = |_, _| Err(cm::Error::Auth("aucun code Steam Guard attendu".into()));
    match cm::sign_in(&config, &auth, &mut prompt) {
        Ok(session) => println!(
            "jeton : ok · steamid {} · {} licence(s)",
            session.steam_id,
            session.licenses.as_ref().map(Vec::len).unwrap_or(0)
        ),
        Err(error) => println!("jeton : échec ({error})"),
    }
}
