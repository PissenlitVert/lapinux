//! Chargeur des plugins natifs.
//!
//! Chaque plugin actif peut déclarer une bibliothèque `.so` dans son manifeste.
//! Le chargeur est le même pour tous : il ouvre le fichier, appelle
//! `lapinux_plugin_entry`, vérifie l'ABI et remet à `init` l'API hôte. Les
//! bibliothèques restent chargées pour toute la durée de la fenêtre, car un
//! plugin peut garder des threads et des pointeurs vers l'API.

use super::theme;
use lapinux_plugin_api::{
    HostApi, PageCommand, PageInput, PageNav, PluginDescriptor, PluginEntry, ThemeSpec,
    HOST_ABI_VERSION, PLUGIN_ENTRY_SYMBOL,
};
use libloading::{Library, Symbol};
use std::collections::{BTreeMap, HashSet};
use std::ffi::CStr;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub(crate) struct NativePlugins {
    loaded: Vec<LoadedPlugin>,
    /// Plugins actifs : une bibliothèque chargée peut être désactivée sans
    /// être déchargée, mais son tick ne doit plus tourner.
    enabled: HashSet<String>,
    /// Un plugin déjà tenté n'est pas rouvert à chaque relecture du thème.
    attempted: HashSet<String>,
}

struct LoadedPlugin {
    id: String,
    /// Garde le code et les données du plugin vivants. Le champ commence par
    /// un underscore parce qu'il n'est jamais lu directement.
    _library: Library,
    draw: Option<unsafe extern "C" fn(*const HostApi)>,
    page: Option<unsafe extern "C" fn(*const HostApi, f32, f32, f64) -> *const std::ffi::c_char>,
    theme: Option<unsafe extern "C" fn(*const HostApi) -> *const std::ffi::c_char>,
    theme_frame:
        Option<unsafe extern "C" fn(*const HostApi, f32, f32, f64) -> *const std::ffi::c_char>,
    home: Option<unsafe extern "C" fn(*const HostApi, f32, f32, f64) -> *const std::ffi::c_char>,
    /// Boîte stable : un plugin peut conserver le pointeur reçu à `init`.
    api: Box<HostApi>,
}

impl NativePlugins {
    /// Charge les bibliothèques déclarées par les plugins actifs. Les plugins
    /// sans `[native]` sont ignorés ; aucun identifiant n'est connu ici.
    pub(crate) fn load(&mut self, plugins: &[theme::Plugin], enabled: &[String], api: &HostApi) {
        self.enabled = enabled.iter().cloned().collect();
        for plugin in plugins {
            let Some(native) = plugin.manifest.native.as_ref() else {
                continue;
            };
            if !self.attempted.insert(plugin.manifest.id.clone()) {
                continue;
            }
            let Some(manifest_path) = plugin.path.as_ref() else {
                eprintln!(
                    "{}",
                    lapinux_core::i18n::t_fmt(
                        "theme.native_manifest_without_path",
                        &[plugin.manifest.id.clone()]
                    )
                );
                continue;
            };
            let path = native_library_path(manifest_path, &native.library);
            match load_plugin(&path, api, &plugin.manifest.id) {
                Ok(loaded) => {
                    eprintln!(
                        "{}",
                        lapinux_core::i18n::t_fmt(
                            "theme.native_loaded",
                            &[plugin.manifest.id.clone(), path.display().to_string()]
                        )
                    );
                    self.loaded.push(loaded);
                }
                Err(error) => eprintln!(
                    "{}",
                    lapinux_core::i18n::t_fmt(
                        "theme.native_load_failed",
                        &[
                            plugin.manifest.id.clone(),
                            path.display().to_string(),
                            error
                        ]
                    )
                ),
            }
        }
    }

    /// Appelle le tick de chaque plugin. C'est le point où un plugin peut lire
    /// la bibliothèque publiée et déposer ses actions, sans dessiner d'egui.
    pub(crate) fn tick(&self) {
        for plugin in &self.loaded {
            if !self.enabled.contains(&plugin.id) {
                continue;
            }
            if let Some(draw) = plugin.draw {
                // SAFETY: l'API et la bibliothèque restent vivantes tant que
                // `LoadedPlugin` existe ; `draw` vient de cette bibliothèque.
                unsafe { draw(&*plugin.api) };
            }
        }
    }

    /// Récupère la frame de la page fournie par un plugin actif. Le plugin
    /// choisit entièrement son contenu ; Lapinux ne connaît que les commandes
    /// génériques de dessin.
    pub(crate) fn page_frame(
        &self,
        plugin_id: &str,
        width: f32,
        height: f32,
        time: f64,
    ) -> Option<Vec<PageCommand>> {
        let plugin = self
            .loaded
            .iter()
            .find(|plugin| plugin.id == plugin_id && self.enabled.contains(&plugin.id))?;
        let page = plugin.page?;
        // SAFETY: la bibliothèque et l'API vivent tant que `LoadedPlugin`
        // existe ; la chaîne renvoyée est valide jusqu'au prochain appel.
        parse_json(unsafe { page(&*plugin.api, width, height, time) })
    }

    /// Thèmes fournis par les bibliothèques chargées. L'identifiant du thème
    /// est celui du plugin : il sert aussi de clé dans la configuration.
    pub(crate) fn theme_specs(&self) -> Vec<(String, ThemeSpec)> {
        let mut specs = Vec::new();
        for plugin in &self.loaded {
            let Some(callback) = plugin.theme else {
                continue;
            };
            // SAFETY: la bibliothèque et l'API vivent tant que le plugin.
            let raw = unsafe { callback(&*plugin.api) };
            if raw.is_null() {
                continue;
            }
            if let Some(spec) = parse_json::<ThemeSpec>(raw) {
                specs.push((plugin.id.clone(), spec));
            }
        }
        specs
    }

    /// Frame de fond du thème actif. Contrairement au tick, elle est lue dès
    /// que le thème est choisi, même si le plugin n'est pas actif.
    pub(crate) fn theme_frame(
        &self,
        theme_id: &str,
        width: f32,
        height: f32,
        time: f64,
    ) -> Option<Vec<PageCommand>> {
        let plugin = self.loaded.iter().find(|plugin| plugin.id == theme_id)?;
        let frame = plugin.theme_frame?;
        // SAFETY: la bibliothèque et l'API vivent tant que le plugin.
        parse_json(unsafe { frame(&*plugin.api, width, height, time) })
    }

    /// Indique si un plugin fournit un accueil interactif.
    pub(crate) fn has_home(&self, theme_id: &str) -> bool {
        self.loaded
            .iter()
            .any(|plugin| plugin.id == theme_id && plugin.home.is_some())
    }

    /// Rend l'accueil fourni par le thème actif. Le plugin lit ses entrées par
    /// l'API hôte (`input`) et dépose ses actions comme n'importe quel plugin.
    pub(crate) fn home_frame(
        &self,
        theme_id: &str,
        width: f32,
        height: f32,
        time: f64,
    ) -> Option<Vec<PageCommand>> {
        let plugin = self.loaded.iter().find(|plugin| plugin.id == theme_id)?;
        let home = plugin.home?;
        // SAFETY: la bibliothèque et l'API vivent tant que le plugin.
        parse_json(unsafe { home(&*plugin.api, width, height, time) })
    }
}

/// Décode la chaîne JSON renvoyée par un callback natif. Le pointeur reste
/// valide jusqu'au prochain appel du callback, donc la copie est immédiate.
fn parse_json<T: serde::de::DeserializeOwned>(raw: *const std::ffi::c_char) -> Option<T> {
    if raw.is_null() {
        return None;
    }
    // SAFETY: le contrat des callbacks est une chaîne C UTF-8 terminée.
    let json = unsafe { CStr::from_ptr(raw) }.to_str().ok()?;
    serde_json::from_str(json).ok()
}

fn native_library_path(manifest: &Path, library: &str) -> PathBuf {
    let library = Path::new(library);
    if library.is_absolute() {
        library.to_path_buf()
    } else {
        manifest
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(library)
    }
}

fn load_plugin(path: &Path, api: &HostApi, id: &str) -> Result<LoadedPlugin, String> {
    // SAFETY: charger une bibliothèque exécute son initialisation. Un plugin
    // natif est du code de confiance, comme le reste des plugins Lapinux.
    let library = unsafe { Library::new(path) }.map_err(|error| {
        lapinux_core::i18n::t_fmt("theme.native_library_unreadable", &[error.to_string()])
    })?;
    // SAFETY: le symbole est cherché dans la bibliothèque qui vient d'être
    // chargée ; son type est le contrat `PluginEntry`.
    let entry: Symbol<'_, PluginEntry> =
        unsafe { library.get(PLUGIN_ENTRY_SYMBOL) }.map_err(|error| {
            lapinux_core::i18n::t_fmt("theme.native_entry_missing", &[error.to_string()])
        })?;
    // SAFETY: `entry` est le point d'entrée exporté par le plugin.
    let descriptor = unsafe { entry() };
    if descriptor.is_null() {
        return Err(lapinux_core::i18n::t("theme.native_null_descriptor").to_string());
    }
    // SAFETY: le plugin promet un descripteur valide pendant le chargement.
    let descriptor: &PluginDescriptor = unsafe { &*descriptor };
    if descriptor.abi_version != HOST_ABI_VERSION {
        return Err(lapinux_core::i18n::t_fmt(
            "theme.native_abi_incompatible",
            &[
                descriptor.abi_version.to_string(),
                HOST_ABI_VERSION.to_string(),
            ],
        ));
    }
    // L'adresse de cette boîte ne bouge plus, même si le vecteur des plugins
    // se réalloue quand un autre plugin est activé plus tard.
    let plugin_api = Box::new(*api);
    if let Some(init) = descriptor.init {
        // SAFETY: l'API est allouée de façon stable et reste valide ; `init`
        // vient de la bibliothèque chargée.
        if !unsafe { init(&*plugin_api) } {
            return Err(lapinux_core::i18n::t("theme.native_init_refused").to_string());
        }
    }
    Ok(LoadedPlugin {
        id: id.to_string(),
        _library: library,
        draw: descriptor.draw,
        page: descriptor.page,
        theme: descriptor.theme,
        theme_frame: descriptor.theme_frame,
        home: descriptor.home,
        api: plugin_api,
    })
}

impl super::LapinuxApp {
    /// Charge les plugins natifs actifs. Appelé au démarrage et après chaque
    /// relecture des réglages ; les bibliothèques déjà chargées sont gardées.
    pub(super) fn load_native_plugins(&mut self) {
        let active = self
            .plugin_library
            .active_plugins(&self.config.general.active_plugins);
        let active_ids: Vec<String> = active
            .iter()
            .map(|plugin| plugin.manifest.id.clone())
            .collect();
        let mut candidates = active;
        // Un thème natif doit être chargé même s'il n'est pas coché comme
        // plugin : c'est sa palette qui doit apparaître dans la liste.
        for plugin in &self.plugin_library.plugins {
            let is_theme = plugin
                .manifest
                .native
                .as_ref()
                .map_or(false, |native| native.theme);
            if is_theme
                && !candidates
                    .iter()
                    .any(|entry| entry.manifest.id == plugin.manifest.id)
            {
                candidates.push(plugin.clone());
            }
        }
        let api = self.plugin_host.api();
        self.native_plugins.load(&candidates, &active_ids, &api);
        self.register_native_themes();
    }

    /// Ajoute les thèmes fournis par les `.so` à la bibliothèque de thèmes.
    pub(super) fn register_native_themes(&mut self) {
        let language = lapinux_core::i18n::active().to_string();
        let selected_id = self.config.general.theme.clone();
        let before = self
            .theme_library
            .find(&selected_id)
            .map(|theme| theme.manifest.clone());
        let specs = self.native_plugins.theme_specs();
        for (id, spec) in specs {
            let Some(plugin) = self.plugin_library.find(&id) else {
                continue;
            };
            let manifest =
                theme::ThemeManifest::from_native(&id, &spec, &plugin.manifest, &language);
            self.theme_library.register_native_theme(manifest);
        }
        let selected = self.theme_library.find(&selected_id);
        let selected_is_native = selected.map_or(false, |theme| theme.native);
        let labels_changed = selected.map(|theme| &theme.manifest) != before.as_ref();
        let active_stale =
            !self.active_theme.id.eq_ignore_ascii_case(&selected_id) || !self.active_theme.native;
        if selected_is_native && (labels_changed || active_stale) {
            self.theme_dirty = true;
        }
    }

    /// Peint la frame de fond du thème natif actif. Un thème TOML classique
    /// n'en a pas et garde le fond habituel.
    pub(super) fn paint_theme_frame(&self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let time = ui.input(|input| input.time);
        if let Some(commands) = self.native_plugins.theme_frame(
            &self.active_theme.id,
            rect.width(),
            rect.height(),
            time,
        ) {
            super::plugin_page::paint_page(ui, rect.min, &commands);
        }
    }

    /// Remplace la page Bibliothèque par la vue du thème natif quand son `.so`
    /// en fournit une. Aucune disposition de bibliothèque n'est codée ici.
    pub(super) fn native_library_page(&mut self, ui: &mut egui::Ui) -> bool {
        let id = self.active_theme.id.clone();
        if !self.active_theme.native || !self.native_plugins.has_home(&id) {
            return false;
        }
        let rect = ui.max_rect();
        let input = self.page_input(ui, rect);
        self.plugin_host.set_input(input);
        let time = ui.input(|input| input.time);
        let Some(commands) = self
            .native_plugins
            .home_frame(&id, rect.width(), rect.height(), time)
        else {
            return false;
        };
        ui.ctx().request_repaint();
        // La vue native peint ses propres jaquettes : sans ce budget, le
        // cache garde les lettres de repli et ne lance aucun chargement.
        self.covers.begin_frame(6);
        self.paint_plugin_commands(ui, rect.min, &commands);
        true
    }

    /// Une bibliothèque native remplace aussi la barre de sections : le plugin
    /// fournit sa propre navigation.
    pub(super) fn native_library_available(&self) -> bool {
        self.page == super::Page::Library
            && self.active_theme.native
            && self.native_plugins.has_home(&self.active_theme.id)
    }

    pub(super) fn page_input(&self, ui: &egui::Ui, rect: egui::Rect) -> PageInput {
        let (keys, mouse, clicked, double_clicked, scroll, time) = ui.input(|input| {
            let mut keys = Vec::new();
            for (key, name) in [
                (egui::Key::ArrowLeft, "left"),
                (egui::Key::ArrowRight, "right"),
                (egui::Key::ArrowUp, "up"),
                (egui::Key::ArrowDown, "down"),
                (egui::Key::Enter, "enter"),
                (egui::Key::Escape, "escape"),
            ] {
                if input.key_pressed(key) {
                    keys.push(name.to_string());
                }
            }
            let mouse = input
                .pointer
                .hover_pos()
                .map(|position| [position.x - rect.left(), position.y - rect.top()]);
            (
                keys,
                mouse,
                input.pointer.primary_clicked(),
                input
                    .pointer
                    .button_double_clicked(egui::PointerButton::Primary),
                input.smooth_scroll_delta.y,
                input.time,
            )
        });
        let mut nav: Vec<PageNav> = super::visible_topbar_pages(
            &self.config.general.topbar_order,
            &self.config.general.topbar_hidden,
        )
        .into_iter()
        .map(|page| PageNav {
            key: page.key().to_string(),
            label: super::page_label(page).to_string(),
        })
        .collect();
        if self.active_theme.plugin_page.is_some() && !nav.iter().any(|entry| entry.key == "plugin")
        {
            nav.push(PageNav {
                key: "plugin".to_string(),
                label: super::page_label(super::Page::Plugin).to_string(),
            });
        }
        let mut labels = BTreeMap::new();
        labels.insert(
            "play".to_string(),
            lapinux_core::i18n::t("library.action.play").to_string(),
        );
        labels.insert(
            "view".to_string(),
            lapinux_core::i18n::t("shell.view_page").to_string(),
        );
        labels.insert(
            "settings".to_string(),
            lapinux_core::i18n::t("shell.settings").to_string(),
        );
        labels.insert(
            "close".to_string(),
            lapinux_core::i18n::t("shell.close_app").to_string(),
        );
        labels.insert(
            "empty".to_string(),
            lapinux_core::i18n::t("shell.empty_library").to_string(),
        );
        labels.insert(
            "open".to_string(),
            lapinux_core::i18n::t("shell.open").to_string(),
        );
        PageInput {
            width: rect.width(),
            height: rect.height(),
            time,
            now: lapinux_core::util::now_unix(),
            keys,
            mouse,
            clicked,
            double_clicked,
            scroll,
            nav,
            labels,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::plugin_host::PluginHost;
    use super::*;
    use lapinux_plugin_api::{HostAction, LibraryGame, PageText, ThemeColors};

    #[test]
    fn un_chemin_relatif_suit_le_manifeste() {
        let path = native_library_path(Path::new("/tmp/plugins/test/plugin.toml"), "libtest.so");
        assert_eq!(path, Path::new("/tmp/plugins/test/libtest.so"));
        let path = native_library_path(Path::new("/tmp/plugins/plugin.toml"), "/opt/a.so");
        assert_eq!(path, Path::new("/opt/a.so"));
    }

    #[test]
    fn charge_le_plugin_de_test_et_recupere_ses_actions() {
        let Ok(path) = std::env::var("LAPINUX_TEST_PLUGIN_SO") else {
            return;
        };
        let host = PluginHost::new();
        host.set_library(&[LibraryGame {
            appid: 7,
            name: "Jeu test".into(),
            ..LibraryGame::default()
        }]);
        let api = host.api();
        let manifest = theme::PluginManifest {
            id: "test-native".into(),
            name: "Test natif".into(),
            native: Some(theme::NativePlugin {
                library: path,
                theme: false,
            }),
            ..theme::PluginManifest::default()
        };
        let plugin = theme::Plugin {
            manifest,
            path: Some(PathBuf::from("/tmp/lapinux-plugin-test.toml")),
        };
        let mut plugins = NativePlugins::default();
        plugins.load(&[plugin], &["test-native".into()], &api);
        assert_eq!(plugins.loaded.len(), 1, "le .so de test doit être chargé");
        plugins.tick();
        let actions = host.take_actions();
        assert!(
            actions.iter().any(|action| matches!(
                action,
                HostAction::Navigate { page } if page == "library"
            )),
            "le plugin doit naviguer vers la bibliothèque : {actions:?}"
        );
        assert!(
            actions
                .iter()
                .any(|action| matches!(action, HostAction::Notify { .. })),
            "le plugin doit avoir lu la bibliothèque avant de naviguer : {actions:?}"
        );

        // Le plugin désactivé ne reçoit plus de tick, mais sa bibliothèque
        // reste chargée : la décharger pendant que du code du plugin vit
        // encore serait dangereux.
        plugins.load(&[], &[], &api);
        assert_eq!(plugins.loaded.len(), 1);
        assert!(plugins.enabled.is_empty());
    }

    #[test]
    fn le_plugin_chat_natif_fournit_toute_sa_page() {
        let Ok(path) = std::env::var("LAPINUX_TEST_CHAT_SO") else {
            return;
        };
        let host = PluginHost::new();
        host.set_page(PageText {
            language: "en".into(),
            title: "Meow".into(),
            subtitle: "It's raining meows.".into(),
            word: "meow".into(),
            footer: "=^.^=".into(),
            colors: ThemeColors {
                accent: [10, 20, 30, 255],
                text: [40, 50, 60, 255],
                text_dim: [70, 80, 90, 255],
                ..ThemeColors::default()
            },
        });
        let api = host.api();
        let manifest = theme::PluginManifest {
            id: "chat-natif".into(),
            name: "Chat".into(),
            native: Some(theme::NativePlugin {
                library: path,
                theme: false,
            }),
            page: Some(theme::PluginPage {
                title: "Miaou".into(),
                ..theme::PluginPage::default()
            }),
            ..theme::PluginManifest::default()
        };
        let mut plugins = NativePlugins::default();
        plugins.load(
            &[theme::Plugin {
                manifest,
                path: Some(PathBuf::from("/tmp/lapinux-plugin-chat.toml")),
            }],
            &["chat-natif".into()],
            &api,
        );
        let frame = plugins
            .page_frame("chat-natif", 800.0, 600.0, 1.5)
            .expect("la page native doit être rendue");
        assert!(frame.iter().any(|command| matches!(
            command,
            lapinux_plugin_api::PageCommand::Text { text, .. } if text == "Meow"
        )));
        assert!(frame.iter().any(|command| matches!(
            command,
            lapinux_plugin_api::PageCommand::Text { text, .. } if text == "meow"
        )));
        assert!(!frame.iter().any(|command| matches!(
            command,
            lapinux_plugin_api::PageCommand::Text { text, .. } if text == "Miaou"
        )));
        assert!(frame.iter().any(|command| matches!(
            command,
            lapinux_plugin_api::PageCommand::Text { text, color, .. }
                if text == "Meow" && color == &[40, 50, 60, 255]
        )));
        assert!(frame.iter().any(|command| matches!(
            command,
            lapinux_plugin_api::PageCommand::Text { color, .. }
                if color[..3] == [10, 20, 30]
        )));
        assert!(frame.len() > 50);
    }

    #[test]
    fn le_theme_natif_fournit_sa_palette_et_sa_frame() {
        let Ok(path) = std::env::var("LAPINUX_TEST_THEME_SO") else {
            return;
        };
        let host = PluginHost::new();
        let api = host.api();
        let manifest = theme::PluginManifest {
            id: "theme-aurora".into(),
            name: "Aurora".into(),
            native: Some(theme::NativePlugin {
                library: path,
                theme: true,
            }),
            ..theme::PluginManifest::default()
        };
        let plugin = theme::Plugin {
            manifest,
            path: Some(PathBuf::from("/tmp/lapinux-plugin-theme-aurora.toml")),
        };
        let mut plugins = NativePlugins::default();
        // Le thème n'est pas coché comme plugin actif : il doit quand même
        // fournir sa palette et sa frame.
        plugins.load(&[plugin], &[], &api);
        let specs = plugins.theme_specs();
        let (id, spec) = specs
            .iter()
            .find(|(id, _)| id == "theme-aurora")
            .expect("le thème natif doit être découvert");
        assert_eq!(id, "theme-aurora");
        assert_eq!(spec.colors.accent, [116, 236, 214, 255]);

        let frame = plugins
            .theme_frame("theme-aurora", 800.0, 600.0, 2.0)
            .expect("le thème doit dessiner sa frame");
        assert!(matches!(
            frame.first(),
            Some(lapinux_plugin_api::PageCommand::Rect { width, height, .. })
                if (*width - 800.0).abs() < 0.1 && (*height - 600.0).abs() < 0.1
        ));
        assert!(frame
            .iter()
            .any(|command| matches!(command, lapinux_plugin_api::PageCommand::Circle { .. })));
    }

    #[test]
    fn le_theme_nx_natif_fournit_son_accueil_et_ses_entrees() {
        let Ok(path) = std::env::var("LAPINUX_TEST_THEME_NX_SO") else {
            return;
        };
        let host = PluginHost::new();
        let api = host.api();
        let manifest = theme::PluginManifest {
            id: "nx".into(),
            name: "NX".into(),
            native: Some(theme::NativePlugin {
                library: path,
                theme: true,
            }),
            ..theme::PluginManifest::default()
        };
        let mut plugins = NativePlugins::default();
        plugins.load(
            &[theme::Plugin {
                manifest,
                path: Some(PathBuf::from("/tmp/lapinux-plugin-theme-nx.toml")),
            }],
            &[],
            &api,
        );
        let specs = plugins.theme_specs();
        let (_, spec) = specs
            .iter()
            .find(|(id, _)| id == "nx")
            .expect("le thème NX natif doit être découvert");
        assert_eq!(spec.colors.accent, [0, 200, 224, 255]);
        assert!(plugins.theme_frame("nx", 640.0, 480.0, 0.0).is_some());
        assert!(plugins.has_home("nx"));

        host.set_library(&[LibraryGame {
            appid: 42,
            name: "Jeu NX".into(),
            installed: true,
            ..LibraryGame::default()
        }]);
        host.set_input(PageInput {
            width: 800.0,
            height: 600.0,
            now: 12 * 3600,
            nav: vec![PageNav {
                key: "home".into(),
                label: "Accueil".into(),
            }],
            ..PageInput::default()
        });
        let home = plugins
            .home_frame("nx", 800.0, 600.0, 0.0)
            .expect("le thème NX doit rendre son accueil");
        assert!(home.iter().any(|command| matches!(
            command,
            lapinux_plugin_api::PageCommand::Cover { appid, .. } if *appid == 42
        )));
        assert!(home.iter().any(|command| matches!(
            command,
            lapinux_plugin_api::PageCommand::NavIcon { page, .. } if page == "home"
        )));
        assert!(home.iter().any(|command| matches!(
            command,
            lapinux_plugin_api::PageCommand::Text { text, .. } if text == "12:00"
        )));

        host.set_input(PageInput {
            width: 800.0,
            height: 600.0,
            now: 12 * 3600,
            keys: vec!["enter".into()],
            ..PageInput::default()
        });
        let _ = plugins.home_frame("nx", 800.0, 600.0, 0.0);
        assert!(host
            .take_actions()
            .iter()
            .any(|action| matches!(action, HostAction::Launch { appid: 42, .. })));
    }
}
