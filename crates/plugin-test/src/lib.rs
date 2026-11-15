//! Plugin natif de test : il lit la bibliothèque une première fois, affiche
//! le nombre de jeux reçus, puis demande la page Bibliothèque.

use lapinux_plugin_api::{HostApi, PluginDescriptor, HOST_ABI_VERSION};
use std::sync::atomic::{AtomicBool, Ordering};

static NAVIGATED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn init(api: *const HostApi) -> bool {
    if api.is_null() {
        return false;
    }
    // SAFETY: l'hôte passe une API valide pendant l'initialisation.
    unsafe { (*api).is_compatible() }
}

unsafe extern "C" fn draw(api: *const HostApi) {
    if api.is_null() || NAVIGATED.swap(true, Ordering::SeqCst) {
        return;
    }
    // SAFETY: le chargeur garde l'API vivante pendant le tick.
    let api = unsafe { &*api };
    let message = match api.library() {
        Ok(games) => format!(
            "Plugin de test : {} jeu(x) dans la bibliothèque",
            games.len()
        ),
        Err(error) => format!("Plugin de test : lecture impossible ({error})"),
    };
    let _ = api.notify(&message);
    let _ = api.navigate("library");
}

#[no_mangle]
pub extern "C" fn lapinux_plugin_entry() -> *const PluginDescriptor {
    // Le descripteur est créé une fois et vit aussi longtemps que la
    // bibliothèque : le chargeur ne le libère pas.
    Box::into_raw(Box::new(PluginDescriptor {
        abi_version: HOST_ABI_VERSION,
        init: Some(init),
        draw: Some(draw),
        page: None,
        theme: None,
        theme_frame: None,
        home: None,
    }))
}
