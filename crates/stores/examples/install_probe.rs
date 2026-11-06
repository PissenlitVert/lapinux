//! Sonde temporaire : plan d'installation d'une boutique et premier mégaoctet.
use lapinux_core::stores::{load_tokens, Store};
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let store = match args.next().as_deref() {
        Some("epic") => Store::Epic,
        Some("gog") => Store::Gog,
        Some("itch") => Store::Itch,
        other => {
            eprintln!("boutique inconnue : {other:?}");
            return;
        }
    };
    let id = args.next().expect("identifiant");
    let platform = args.next().unwrap_or_else(|| "linux".into());
    let cdn = args.next().unwrap_or_default();
    let paths = lapinux_core::paths::LapinuxPaths::discover();
    let tokens = load_tokens(&paths.data_dir, store).expect("jetons");
    println!("compte {:?} / {}", store, tokens.account);
    let plan = match lapinux_stores::install_plan(store, &tokens, &id, &platform, &cdn) {
        Ok(plan) => plan,
        Err(error) => {
            println!("PLAN ÉCHOUÉ : {error}");
            return;
        }
    };
    println!(
        "plan {} · {} · {} · {} pas · {} octets",
        plan.name,
        plan.installdir,
        plan.platform,
        plan.steps.len(),
        plan.size
    );
    let cache = PathBuf::from("/tmp/lapinux-probe-cache");
    let dest = PathBuf::from("/tmp/lapinux-probe-dest");
    let _ = std::fs::remove_dir_all(&dest);
    let mut seen = 0u64;
    let result = lapinux_stores::install(&tokens, &plan, &cache, &dest, &mut |done, total| {
        let first = seen == 0;
        seen = done;
        if first {
            println!("premier retour : {done} / {total}");
        }
        // On coupe tôt : la sonde ne télécharge pas un jeu entier.
        done < 32 * 1024 * 1024
    });
    match result {
        Ok(()) => println!("install terminé ou interrompu proprement ({seen} octets)"),
        Err(error) => println!("INSTALL ÉCHOUÉE : {error}"),
    }
}
