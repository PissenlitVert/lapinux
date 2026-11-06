//! Traduction des touches Wayland en texte, avec la disposition du clavier.
//!
//! `libxkbcommon.so.0` est présente sur les systèmes Wayland, mais le lien
//! compile-time exigerait le paquet de développement. La bibliothèque est donc
//! chargée à l'exécution : un système sans xkbcommon garde un repli ASCII pour
//! les lettres, sans empêcher l'overlay de démarrer.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::OnceLock;

pub const KEY_ESCAPE: u32 = 0xff1b;
pub const KEY_RETURN: u32 = 0xff0d;
pub const KEY_BACKSPACE: u32 = 0xff08;

#[derive(Clone, Copy)]
struct Functions {
    context_new: unsafe extern "C" fn(u32) -> *mut c_void,
    context_unref: unsafe extern "C" fn(*mut c_void),
    keymap_new_from_string: unsafe extern "C" fn(
        *mut c_void,
        *const c_char,
        u32,
        u32,
    ) -> *mut c_void,
    keymap_unref: unsafe extern "C" fn(*mut c_void),
    state_new: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    state_unref: unsafe extern "C" fn(*mut c_void),
    state_update_mask:
        unsafe extern "C" fn(*mut c_void, u32, u32, u32, u32, u32, u32) -> c_int,
    state_key_get_utf8:
        unsafe extern "C" fn(*mut c_void, u32, *mut c_char, usize) -> c_int,
    state_key_get_one_sym: unsafe extern "C" fn(*mut c_void, u32) -> u32,
}

unsafe fn symbol<T: Copy>(library: *mut c_void, name: &CStr) -> Option<T> {
    let pointer = libc::dlsym(library, name.as_ptr());
    if pointer.is_null() {
        return None;
    }
    Some(std::mem::transmute_copy(&pointer))
}

fn functions() -> Option<Functions> {
    static FUNCTIONS: OnceLock<Option<Functions>> = OnceLock::new();
    *FUNCTIONS.get_or_init(|| unsafe {
        let library_name = CString::new("libxkbcommon.so.0").ok()?;
        let library = libc::dlopen(library_name.as_ptr(), libc::RTLD_NOW);
        if library.is_null() {
            return None;
        }
        Some(Functions {
            context_new: symbol(library, c"xkb_context_new")?,
            context_unref: symbol(library, c"xkb_context_unref")?,
            keymap_new_from_string: symbol(library, c"xkb_keymap_new_from_string")?,
            keymap_unref: symbol(library, c"xkb_keymap_unref")?,
            state_new: symbol(library, c"xkb_state_new")?,
            state_unref: symbol(library, c"xkb_state_unref")?,
            state_update_mask: symbol(library, c"xkb_state_update_mask")?,
            state_key_get_utf8: symbol(library, c"xkb_state_key_get_utf8")?,
            state_key_get_one_sym: symbol(library, c"xkb_state_key_get_one_sym")?,
        })
    })
}

/// État clavier xkbcommon : la table de touches du compositeur et les
/// modificateurs courants.
pub struct Xkb {
    functions: Functions,
    context: *mut c_void,
    keymap: *mut c_void,
    state: *mut c_void,
}

impl Xkb {
    /// Construit l'état depuis la keymap textuelle envoyée par Wayland.
    pub fn from_keymap(keymap: &[u8]) -> Option<Self> {
        let functions = functions()?;
        // Le protocole Wayland inclut l'octet nul final ; il ne doit pas
        // produire un zéro au milieu de la chaîne C.
        let keymap = keymap.strip_suffix(&[0]).unwrap_or(keymap);
        let keymap = CString::new(keymap).ok()?;
        unsafe {
            let context = (functions.context_new)(0);
            if context.is_null() {
                return None;
            }
            const XKB_KEYMAP_FORMAT_TEXT_V1: u32 = 1;
            let map = (functions.keymap_new_from_string)(
                context,
                keymap.as_ptr(),
                XKB_KEYMAP_FORMAT_TEXT_V1,
                0,
            );
            if map.is_null() {
                (functions.context_unref)(context);
                return None;
            }
            let state = (functions.state_new)(map);
            if state.is_null() {
                (functions.keymap_unref)(map);
                (functions.context_unref)(context);
                return None;
            }
            Some(Self {
                functions,
                context,
                keymap: map,
                state,
            })
        }
    }

    /// Applique les masques du protocole Wayland après un événement
    /// `Modifiers`, pour que Maj et AltGr produisent les bons caractères.
    pub fn update_mask(&mut self, depressed: u32, latched: u32, locked: u32, group: u32) {
        unsafe {
            (self.functions.state_update_mask)(
                self.state, depressed, latched, locked, 0, 0, group,
            );
        }
    }

    /// Texte produit par la touche, selon la disposition et les modificateurs.
    pub fn text(&self, keycode: u32) -> Option<String> {
        let mut buffer = [0u8; 64];
        let length = unsafe {
            (self.functions.state_key_get_utf8)(
                self.state,
                keycode,
                buffer.as_mut_ptr() as *mut c_char,
                buffer.len(),
            )
        };
        if length <= 0 || length as usize >= buffer.len() {
            return None;
        }
        String::from_utf8(buffer[..length as usize].to_vec()).ok()
    }

    /// Touche symbolique, pour distinguer Entrée, Échap et Retour arrière.
    pub fn keysym(&self, keycode: u32) -> u32 {
        unsafe { (self.functions.state_key_get_one_sym)(self.state, keycode) }
    }
}

impl Drop for Xkb {
    fn drop(&mut self) {
        unsafe {
            (self.functions.state_unref)(self.state);
            (self.functions.keymap_unref)(self.keymap);
            (self.functions.context_unref)(self.context);
        }
    }
}

/// Repli sans xkbcommon : les lettres et les chiffres restent saisissables
/// sur une disposition QWERTY. La disposition française demande xkbcommon.
pub fn fallback_text(keycode: u32) -> Option<char> {
    let ch = match keycode {
        2 => '1', 3 => '2', 4 => '3', 5 => '4', 6 => '5',
        7 => '6', 8 => '7', 9 => '8', 10 => '9', 11 => '0',
        12 => '-', 13 => '=', 16 => 'q', 17 => 'w', 18 => 'e',
        19 => 'r', 20 => 't', 21 => 'y', 22 => 'u', 23 => 'i',
        24 => 'o', 25 => 'p', 26 => '[', 27 => ']', 30 => 'a',
        31 => 's', 32 => 'd', 33 => 'f', 34 => 'g', 35 => 'h',
        36 => 'j', 37 => 'k', 38 => 'l', 39 => ';', 40 => '\'',
        41 => '`', 43 => '\\', 44 => 'z', 45 => 'x', 46 => 'c',
        47 => 'v', 48 => 'b', 49 => 'n', 50 => 'm', 51 => ',',
        52 => '.', 53 => '/', 57 => ' ', 86 => '<',
        _ => return None,
    };
    Some(ch)
}
