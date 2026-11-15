//! API hôte des plugins natifs de Lapinux.
//!
//! Un plugin est une bibliothèque partagée qui exporte
//! [`PLUGIN_ENTRY_SYMBOL`] et reçoit une [`HostApi`]. L'API est une structure C
//! versionnée : le plugin vérifie `abi_version` et `struct_size` avant de
//! l'utiliser. Les données passent en JSON pour ne pas figer les structures
//! Rust dans l'ABI.
//!
//! Aucun cas particulier de plugin n'est prévu ici : cette API ne connaît que
//! des actions et des données générales de Lapinux.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::{c_char, c_void, CStr, CString};

/// Version de l'API hôte. Elle change quand une fonction ou une structure de
/// cette bibliothèque change de façon incompatible.
pub const HOST_ABI_VERSION: u32 = 6;

/// Symbole exporté par un plugin natif :
/// `extern "C" fn lapinux_plugin_entry() -> *const PluginDescriptor`.
pub const PLUGIN_ENTRY_SYMBOL: &[u8] = b"lapinux_plugin_entry\0";

/// Jeu tel que l'API le présente à un plugin.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LibraryGame {
    pub appid: u32,
    pub name: String,
    pub installed: bool,
    pub on_disk: bool,
    pub steam: bool,
    pub family: bool,
    pub family_available: bool,
    /// Identifiant de la boutique (`steam`, `epic`, `gog`, `itch`).
    pub store: Option<String>,
    pub cover: String,
    pub platforms: Vec<String>,
    pub mobile: bool,
    pub size: Option<u64>,
    pub publishers: Vec<String>,
    pub developers: Vec<String>,
    pub genres: Vec<String>,
    pub playtime: u64,
    pub acquired: Option<i64>,
    pub updated: i64,
    pub last_played: i64,
    /// Mise à jour Steam disponible, quand elle est connue.
    pub update: Option<u64>,
}

/// Textes de la page du plugin dans la langue active. Le plugin les reçoit
/// depuis son manifeste et ses blocs `[translations]` ; il ne doit pas les
/// recopier dans son code.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PageText {
    pub language: String,
    pub title: String,
    pub subtitle: String,
    /// Mot ou contenu court propre au plugin.
    pub word: String,
    pub footer: String,
    /// Palette du thème actif, déjà résolue par l'application.
    pub colors: ThemeColors,
}

/// Palette du thème actif remise à un plugin natif. Les couleurs sont en
/// RGBA non prémultiplié, comme dans les commandes de dessin.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ThemeColors {
    pub background: [u8; 4],
    pub panel: [u8; 4],
    pub card: [u8; 4],
    pub card_hover: [u8; 4],
    pub card_active: [u8; 4],
    pub border: [u8; 4],
    pub accent: [u8; 4],
    pub text: [u8; 4],
    pub text_dim: [u8; 4],
}

/// Thème fourni par un plugin natif : palette et géométrie. Les libellés
/// viennent du manifeste du plugin et de ses traductions, jamais du `.so`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ThemeSpec {
    pub colors: ThemeColors,
    pub rounding: f32,
    pub font_scale: f32,
}

/// Entrée d'une page de navigation fournie à un plugin d'accueil. Lapinux
/// fournit les clés et les libellés déjà traduits ; le plugin décide seulement
/// de leur disposition et de leur apparence.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PageNav {
    pub key: String,
    pub label: String,
}

/// Entrées clavier/souris et contexte d'une page native interactive.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PageInput {
    pub width: f32,
    pub height: f32,
    /// Temps écoulé depuis le démarrage, en secondes.
    pub time: f64,
    /// Heure Unix, pour l'horloge d'un accueil.
    pub now: i64,
    /// Touches pressées pendant cette image : `left`, `right`, `up`, `down`,
    /// `enter`, `escape`.
    pub keys: Vec<String>,
    /// Position de la souris dans la page, en pixels.
    pub mouse: Option<[f32; 2]>,
    pub clicked: bool,
    pub double_clicked: bool,
    pub scroll: f32,
    /// Pages visibles, dans l'ordre de la barre de navigation.
    pub nav: Vec<PageNav>,
    /// Libellés traduits que l'accueil peut utiliser. Clés usuelles :
    /// `play`, `view`, `settings`, `close`, `empty`, `open`.
    pub labels: BTreeMap<String, String>,
}

/// Action générale qu'un plugin peut demander à Lapinux.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum HostAction {
    /// Ouvre une section : `home`, `library`, `collections`, `news`,
    /// `downloads`, `controllers`, `profile`, `boutiques`, `captures`.
    Navigate { page: String },
    /// Lance un jeu. `dry_run` prépare la commande sans démarrer le jeu.
    Launch {
        appid: u32,
        #[serde(default)]
        dry_run: bool,
    },
    /// Ouvre la fiche d'un jeu, sans le lancer.
    OpenGame { appid: u32 },
    /// Ouvre les réglages sur un onglet par sa clé.
    OpenSettings { tab: String },
    /// Relit la bibliothèque et redemande un scan au daemon.
    RefreshLibrary,
    /// Affiche une notification Lapinux.
    Notify { message: String },
    /// Envoie une requête au daemon. Elle est asynchrone : le plugin ne reçoit
    /// pas la réponse par cette API.
    Daemon { request: serde_json::Value },
    /// Ferme la fenêtre de Lapinux.
    Quit,
}

/// Alignement horizontal d'un texte de page.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PageTextAlign {
    Left,
    Center,
    Right,
}

/// Commande de dessin d'une page fournie par un plugin natif.
///
/// Le plugin reçoit les dimensions de la page et le temps courant, puis
/// renvoie une liste de commandes en JSON. Lapinux les peint avec egui : le
/// plugin n'a pas besoin de connaître egui ni la disposition interne de l'UI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PageCommand {
    Text {
        x: f32,
        y: f32,
        text: String,
        size: f32,
        color: [u8; 4],
        #[serde(default = "default_text_align")]
        align: PageTextAlign,
    },
    Rect {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        #[serde(default)]
        rounding: f32,
        color: [u8; 4],
    },
    Circle {
        x: f32,
        y: f32,
        radius: f32,
        color: [u8; 4],
    },
    Line {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        width: f32,
        color: [u8; 4],
    },
    /// Jaquette d'un jeu de la bibliothèque. L'hôte choisit la meilleure image
    /// disponible et se rabat sur une carte neutre si elle manque.
    Cover {
        appid: u32,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        #[serde(default)]
        rounding: f32,
    },
    /// Pictogramme d'une section de navigation. L'hôte le dessine avec les
    /// icônes vectorielles communes, pour que le dock d'un thème natif garde
    /// exactement les mêmes repères que la barre classique.
    NavIcon {
        page: String,
        x: f32,
        y: f32,
        size: f32,
        color: [u8; 4],
    },
}

fn default_text_align() -> PageTextAlign {
    PageTextAlign::Center
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HostApi {
    /// Doit valoir [`HOST_ABI_VERSION`].
    pub abi_version: u32,
    /// Taille de cette structure, pour qu'un plugin ancien puisse vérifier
    /// qu'il connaît bien tous les champs.
    pub struct_size: usize,
    /// Pointeur opaque fourni par Lapinux ; toujours le passer aux callbacks.
    pub context: *mut c_void,
    /// Renvoie un tableau JSON de [`LibraryGame`]. Lapinux alloue la chaîne ;
    /// le plugin la libère avec `free_string`. Renvoie null en cas d'échec.
    pub library_json: Option<unsafe extern "C" fn(*mut c_void) -> *mut c_char>,
    /// Reçoit un [`HostAction`] sérialisé en JSON. Renvoie 1 si l'action a été
    /// acceptée, 0 sinon.
    pub action: Option<unsafe extern "C" fn(*mut c_void, *const c_char) -> u32>,
    /// Libère une chaîne renvoyée par Lapinux.
    pub free_string: Option<unsafe extern "C" fn(*mut c_void, *mut c_char)>,
    /// Renvoie les textes localisés de la page active sous forme d'objet JSON
    /// [`PageText`]. Lapinux alloue la chaîne ; le plugin la libère avec
    /// `free_string`.
    pub page_json: Option<unsafe extern "C" fn(*mut c_void) -> *mut c_char>,
    /// Renvoie l'état des entrées de la page native active sous forme d'objet
    /// JSON [`PageInput`]. Lapinux alloue la chaîne ; le plugin la libère avec
    /// `free_string`.
    pub input_json: Option<unsafe extern "C" fn(*mut c_void) -> *mut c_char>,
}

impl HostApi {
    /// Vérifie que le plugin et l'hôte parlent la même version d'ABI.
    pub fn is_compatible(&self) -> bool {
        self.abi_version == HOST_ABI_VERSION && self.struct_size >= std::mem::size_of::<Self>()
    }

    /// Lit la bibliothèque. Les erreurs de l'hôte sont rendues comme un texte
    /// lisible ; un plugin peut les journaliser ou les afficher.
    pub fn library(&self) -> Result<Vec<LibraryGame>, String> {
        let text = self.read_json(self.library_json, "library_json")?;
        serde_json::from_str(&text).map_err(|error| format!("bibliothèque JSON invalide : {error}"))
    }

    /// Lit les textes localisés de la page active.
    pub fn page(&self) -> Result<PageText, String> {
        let text = self.read_json(self.page_json, "page_json")?;
        serde_json::from_str(&text).map_err(|error| format!("page JSON invalide : {error}"))
    }

    /// Lit les entrées de la page native active.
    pub fn input(&self) -> Result<PageInput, String> {
        let text = self.read_json(self.input_json, "input_json")?;
        serde_json::from_str(&text).map_err(|error| format!("entrées JSON invalides : {error}"))
    }

    fn read_json(
        &self,
        call: Option<unsafe extern "C" fn(*mut c_void) -> *mut c_char>,
        label: &str,
    ) -> Result<String, String> {
        let Some(call) = call else {
            return Err(format!("{label} absent de l'API hôte"));
        };
        // SAFETY: `context` vient de Lapinux et reste valide pendant l'appel.
        let raw = unsafe { call(self.context) };
        if raw.is_null() {
            return Err(format!("{label} indisponible"));
        }
        // SAFETY: l'hôte renvoie une chaîne C allouée jusqu'à free_string.
        let text = unsafe { CStr::from_ptr(raw) }.to_str().map(str::to_owned);
        if let Some(free) = self.free_string {
            // SAFETY: `raw` vient du callback et n'est pas encore libéré.
            unsafe { free(self.context, raw) };
        }
        text.map_err(|_| format!("{label} invalide : UTF-8"))
    }

    /// Envoie une action à Lapinux. L'action est traitée à la prochaine image,
    /// donc un plugin ne bloque pas la boucle d'interface.
    pub fn dispatch(&self, action: &HostAction) -> bool {
        let Some(call) = self.action else {
            return false;
        };
        let Ok(json) = serde_json::to_string(action) else {
            return false;
        };
        let Ok(json) = CString::new(json) else {
            return false;
        };
        // SAFETY: la chaîne est terminée par NUL et reste vivante pendant
        // l'appel ; `context` appartient à l'hôte.
        unsafe { call(self.context, json.as_ptr()) == 1 }
    }

    /// Raccourci pour changer de section.
    pub fn navigate(&self, page: &str) -> bool {
        self.dispatch(&HostAction::Navigate {
            page: page.to_string(),
        })
    }

    /// Raccourci pour lancer un jeu.
    pub fn launch_game(&self, appid: u32, dry_run: bool) -> bool {
        self.dispatch(&HostAction::Launch { appid, dry_run })
    }

    /// Raccourci pour ouvrir un onglet des réglages.
    pub fn open_settings(&self, tab: &str) -> bool {
        self.dispatch(&HostAction::OpenSettings {
            tab: tab.to_string(),
        })
    }

    /// Raccourci pour relire la bibliothèque et relancer un scan.
    pub fn refresh_library(&self) -> bool {
        self.dispatch(&HostAction::RefreshLibrary)
    }

    /// Raccourci pour afficher une notification Lapinux.
    pub fn notify(&self, message: &str) -> bool {
        self.dispatch(&HostAction::Notify {
            message: message.to_string(),
        })
    }

    /// Raccourci pour une requête asynchrone au daemon.
    pub fn daemon(&self, request: serde_json::Value) -> bool {
        self.dispatch(&HostAction::Daemon { request })
    }
}

/// Ce qu'un plugin natif exporte derrière [`PLUGIN_ENTRY_SYMBOL`].
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PluginDescriptor {
    pub abi_version: u32,
    /// Initialisation : reçoit l'API hôte, renvoie true si le plugin démarre.
    pub init: Option<unsafe extern "C" fn(*const HostApi) -> bool>,
    /// Appelé à chaque image : le plugin peut lire l'API et déposer des
    /// actions. Il ne dessine pas directement dans l'interface egui.
    pub draw: Option<unsafe extern "C" fn(*const HostApi)>,
    /// Rend la page du plugin. Reçoit la largeur, la hauteur et le temps en
    /// secondes ; renvoie un tableau JSON de [`PageCommand`]. La chaîne reste
    /// la propriété du plugin et doit rester valide jusqu'au prochain appel.
    pub page: Option<unsafe extern "C" fn(*const HostApi, f32, f32, f64) -> *const c_char>,
    /// Rend le thème du plugin. Renvoie un JSON [`ThemeSpec`] ; la chaîne
    /// reste la propriété du plugin jusqu'au prochain appel.
    pub theme: Option<unsafe extern "C" fn(*const HostApi) -> *const c_char>,
    /// Dessine la frame de fond du thème. Renvoie un tableau JSON de
    /// [`PageCommand`], valide jusqu'au prochain appel.
    pub theme_frame: Option<unsafe extern "C" fn(*const HostApi, f32, f32, f64) -> *const c_char>,
    /// Rend l'accueil interactif fourni par le plugin. Reçoit les dimensions et
    /// le temps ; lit ses entrées avec [`HostApi::input`] et renvoie un tableau
    /// JSON de [`PageCommand`]. La chaîne reste valide jusqu'au prochain appel.
    pub home: Option<unsafe extern "C" fn(*const HostApi, f32, f32, f64) -> *const c_char>,
}

/// Type du point d'entrée exporté par une bibliothèque de plugin.
pub type PluginEntry = unsafe extern "C" fn() -> *const PluginDescriptor;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_actions_ont_un_contrat_json_stable() {
        assert_eq!(
            serde_json::to_value(HostAction::Navigate {
                page: "library".into()
            })
            .unwrap(),
            serde_json::json!({"action": "navigate", "page": "library"})
        );
        let launch: HostAction = serde_json::from_value(serde_json::json!({
            "action": "launch",
            "appid": 42,
            "dry_run": true
        }))
        .unwrap();
        assert_eq!(
            launch,
            HostAction::Launch {
                appid: 42,
                dry_run: true
            }
        );
    }
}
