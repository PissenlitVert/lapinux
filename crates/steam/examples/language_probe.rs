//! Diagnostic : langue de jeu que Steam utiliserait sur cette machine.

fn main() {
    println!("registre : {:?}", lapinux_steam::native::steam_language());
    println!("langue par défaut : {:?}", lapinux_steam::native::default_language());
    println!(
        "libellé : {:?}",
        lapinux_steam::native::default_language_label()
    );
}
