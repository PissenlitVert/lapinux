//! Hôte des plugins natifs : API versionnée et file d'actions.
//!
//! Les callbacks sont appelés par une bibliothèque plugin pendant le dessin.
//! Ils ne touchent jamais directement à `LapinuxApp` : la bibliothèque est
//! publiée dans un état partagé et les actions sont vidées à l'image suivante.
//! Cela évite de prêter l'application deux fois et donne un point d'entrée
//! unique, générique, sans connaître le moindre plugin.

use lapinux_plugin_api::{
    HostAction, HostApi, LibraryGame, PageInput, PageText, ThemeColors, HOST_ABI_VERSION,
};
use std::collections::VecDeque;
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::{Arc, Mutex};

pub(crate) struct PluginHost {
    state: Arc<Mutex<HostState>>,
}

struct HostState {
    library_json: String,
    page_json: String,
    input_json: String,
    actions: VecDeque<HostAction>,
}

impl PluginHost {
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(HostState {
                library_json: "[]".to_string(),
                page_json: "{}".to_string(),
                input_json: "{}".to_string(),
                actions: VecDeque::new(),
            })),
        }
    }

    /// Copie la bibliothèque dans l'état partagé, déjà sérialisée. Le plugin
    /// peut la demander à tout moment sans toucher aux données de l'interface.
    pub(crate) fn set_library(&self, games: &[LibraryGame]) {
        let Ok(json) = serde_json::to_string(games) else {
            return;
        };
        if let Ok(mut state) = self.state.lock() {
            state.library_json = json;
        }
    }

    pub(crate) fn take_actions(&self) -> Vec<HostAction> {
        self.state
            .lock()
            .map(|mut state| state.actions.drain(..).collect())
            .unwrap_or_default()
    }

    /// Publie les textes de la page dans la langue active. Un plugin peut
    /// alors les lire à chaque image sans les coder dans son `.so`.
    pub(crate) fn set_page(&self, text: PageText) {
        let Ok(json) = serde_json::to_string(&text) else {
            return;
        };
        if let Ok(mut state) = self.state.lock() {
            state.page_json = json;
        }
    }

    /// Publie l'état des entrées de la page native active.
    pub(crate) fn set_input(&self, input: PageInput) {
        let Ok(json) = serde_json::to_string(&input) else {
            return;
        };
        if let Ok(mut state) = self.state.lock() {
            state.input_json = json;
        }
    }

    /// Construit l'API à remettre au plugin. Le pointeur de contexte désigne
    /// `self`, qui vit dans un `Box` stable dans `LapinuxApp`.
    pub(crate) fn api(&self) -> HostApi {
        HostApi {
            abi_version: HOST_ABI_VERSION,
            struct_size: std::mem::size_of::<HostApi>(),
            context: self as *const Self as *mut c_void,
            library_json: Some(library_json),
            action: Some(action),
            free_string: Some(free_string),
            page_json: Some(page_json),
            input_json: Some(input_json),
        }
    }
}

unsafe extern "C" fn library_json(context: *mut c_void) -> *mut c_char {
    if context.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: le contexte est le `PluginHost` vivant fourni par `api`.
    let host = unsafe { &*(context as *const PluginHost) };
    let Ok(state) = host.state.lock() else {
        return std::ptr::null_mut();
    };
    CString::new(state.library_json.clone())
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}

unsafe extern "C" fn action(context: *mut c_void, json: *const c_char) -> u32 {
    if context.is_null() || json.is_null() {
        return 0;
    }
    // SAFETY: les deux pointeurs sont valides pendant l'appel.
    let host = unsafe { &*(context as *const PluginHost) };
    let Ok(json) = unsafe { CStr::from_ptr(json) }.to_str() else {
        return 0;
    };
    let Ok(action) = serde_json::from_str::<HostAction>(json) else {
        return 0;
    };
    let Ok(mut state) = host.state.lock() else {
        return 0;
    };
    state.actions.push_back(action);
    1
}

unsafe extern "C" fn page_json(context: *mut c_void) -> *mut c_char {
    if context.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: le contexte est le `PluginHost` vivant fourni par `api`.
    let host = unsafe { &*(context as *const PluginHost) };
    let Ok(state) = host.state.lock() else {
        return std::ptr::null_mut();
    };
    CString::new(state.page_json.clone())
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}

unsafe extern "C" fn input_json(context: *mut c_void) -> *mut c_char {
    if context.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: le contexte est le `PluginHost` vivant fourni par `api`.
    let host = unsafe { &*(context as *const PluginHost) };
    let Ok(state) = host.state.lock() else {
        return std::ptr::null_mut();
    };
    CString::new(state.input_json.clone())
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}

impl super::LapinuxApp {
    /// Publie la bibliothèque de l'interface vers l'API native.
    pub(super) fn sync_plugin_library(&mut self) {
        let entries = self.library_entries_all();
        let games: Vec<LibraryGame> = entries.iter().map(|entry| entry.plugin_game()).collect();
        self.plugin_host.set_library(&games);
    }

    /// Publie la page active dans la langue courante. Le plugin natif lit ces
    /// textes à chaque image au lieu de les recopier dans son `.so`.
    pub(super) fn sync_plugin_page_text(&self) {
        let colors = self.active_theme.colors;
        let text = self
            .active_theme
            .plugin_page
            .as_ref()
            .map(|page| PageText {
                language: lapinux_core::i18n::active().to_string(),
                title: page.title.clone(),
                subtitle: page.subtitle.clone(),
                word: page.word.clone(),
                footer: page.footer.clone(),
                colors: ThemeColors {
                    background: color_rgba(colors.background),
                    panel: color_rgba(colors.panel),
                    card: color_rgba(colors.card),
                    card_hover: color_rgba(colors.card_hover),
                    card_active: color_rgba(colors.card_active),
                    border: color_rgba(colors.border),
                    accent: color_rgba(colors.accent),
                    text: color_rgba(colors.text),
                    text_dim: color_rgba(colors.text_dim),
                },
            })
            .unwrap_or_default();
        self.plugin_host.set_page(text);
    }

    /// Applique les actions déposées par les plugins. Elles passent par les
    /// mêmes fonctions que les contrôles de l'interface.
    pub(super) fn drain_plugin_actions(&mut self, ctx: &egui::Context) {
        for action in self.plugin_host.take_actions() {
            match action {
                HostAction::Navigate { page } => {
                    let Some(page) = super::Page::from_key(&page) else {
                        continue;
                    };
                    self.page = page;
                    self.collection_detail_open = false;
                    self.selected = None;
                    if self.settings_open {
                        self.close_settings();
                    }
                }
                HostAction::Launch { appid, dry_run } => self.launch(appid, dry_run),
                HostAction::OpenGame { appid } => {
                    self.page = super::Page::Library;
                    self.collection_detail_open = false;
                    self.selected = Some(appid);
                    if self.settings_open {
                        self.close_settings();
                    }
                }
                HostAction::OpenSettings { tab } => {
                    if let Some(tab) = super::SettingsTab::from_key(&tab) {
                        self.open_settings(tab);
                    }
                }
                HostAction::RefreshLibrary => self.refresh_async(ctx, true),
                HostAction::Notify { message } => self.notify(message),
                HostAction::Daemon { request } => {
                    let Ok(request) = serde_json::from_value::<lapinux_core::ipc::Req>(request)
                    else {
                        continue;
                    };
                    let client = self.client.detached();
                    std::thread::spawn(move || {
                        let _ = client.request(&request);
                    });
                }
                HostAction::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            }
        }
    }
}

fn color_rgba(color: egui::Color32) -> [u8; 4] {
    [color.r(), color.g(), color.b(), color.a()]
}

unsafe extern "C" fn free_string(_context: *mut c_void, text: *mut c_char) {
    if text.is_null() {
        return;
    }
    // SAFETY: cette chaîne a été créée par `library_json` avec CString::into_raw.
    drop(unsafe { CString::from_raw(text) });
}

#[cfg(test)]
mod tests {
    use super::*;
    use lapinux_plugin_api::HostAction;

    #[test]
    fn le_plugin_lit_la_bibliotheque_et_envoie_des_actions() {
        let host = PluginHost::new();
        host.set_library(&[LibraryGame {
            appid: 42,
            name: "Jeu".into(),
            installed: true,
            ..LibraryGame::default()
        }]);
        let api = host.api();
        assert!(api.is_compatible());
        let games = api.library().expect("bibliothèque JSON");
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].appid, 42);

        assert!(api.navigate("library"));
        assert!(api.launch_game(42, true));
        assert_eq!(
            host.take_actions(),
            vec![
                HostAction::Navigate {
                    page: "library".into()
                },
                HostAction::Launch {
                    appid: 42,
                    dry_run: true
                }
            ]
        );
    }
}
