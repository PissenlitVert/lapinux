//! Configurations de jeux : catalogue publié par Pissenlit Vert, installation
//! et retrait des fichiers et réglages qu'elles décrivent.
//!
//! Rien n'est exécuté : une configuration ne pose que des réglages et des
//! fichiers. Chaque fichier remplacé est sauvegardé, chaque variable touchée
//! garde sa valeur d'avant, et l'installation entière est annulée si une
//! étape échoue.

use lapinux_core::configurations::{
    self, Catalog, Configuration, ConfigurationSet, FileSpec, InstallReport,
    InstalledConfiguration, InstalledFile, PluginCatalogSet, PluginEntry, ThemeCatalogSet,
    ThemeEntry,
};
use lapinux_core::ipc::Envelope;
use lapinux_core::model::Game;
use lapinux_plugin_api::{PluginEntry as NativePluginEntry, HOST_ABI_VERSION, PLUGIN_ENTRY_SYMBOL};
use libloading::{Library, Symbol};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Délai entre deux lectures du catalogue distant. Court : un plugin ou un
/// thème ajouté au dépôt doit apparaître sans mise à jour de Lapinux.
const MAX_AGE: i64 = 30 * 60;
/// Plafond d'un fichier téléchargé.
const MAX_FILE: u64 = 20 * 1024 * 1024;
/// Plafond d'une configuration entière.
const MAX_TOTAL: u64 = 100 * 1024 * 1024;
/// Clé de la base où vivent les configurations posées, par jeu.
const RECORDS: &str = "configurations";

/// Le catalogue gardé en mémoire par le daemon.
#[derive(Default)]
pub struct State {
    catalog: Catalog,
    remote: bool,
    fetched: i64,
    last_try: i64,
    running: bool,
    /// Prochaine lecture forcée : l'URL reçoit un paramètre unique pour
    /// contourner le cache du CDN, qui servait sinon le catalogue d'avant.
    force: bool,
    /// Adresse d'où vient le catalogue : elle sert à retrouver les fichiers.
    url: String,
}

impl State {
    pub fn new() -> Self {
        Self {
            catalog: configurations::bundled(),
            url: configurations::catalog_url(),
            ..Default::default()
        }
    }

    /// Configurations qui désignent ce jeu, retirées comprises : l'interface
    /// montre les installées même quand le catalogue ne les propose plus.
    pub fn for_game(&self, game: &Game) -> Vec<Configuration> {
        self.catalog
            .configurations
            .iter()
            .filter(|entry| entry.applies_to(game))
            .cloned()
            .collect()
    }

    fn stale(&self) -> bool {
        self.fetched <= 0
            || lapinux_core::util::now_unix() - self.last_try.max(self.fetched) > MAX_AGE
    }
}

/// Relit le catalogue publié. `force` ajoute un paramètre unique à l'URL :
/// sans lui, le CDN de GitHub peut rendre la copie précédente pendant
/// plusieurs minutes après une publication.
fn fetch(force: bool) -> Result<(Catalog, String), String> {
    let base = configurations::catalog_url();
    let url = if force {
        let separator = if base.contains('?') { '&' } else { '?' };
        format!(
            "{base}{separator}refresh={}",
            lapinux_core::util::now_unix()
        )
    } else {
        base.clone()
    };
    let response = ureq::get(&url)
        .set("User-Agent", "Lapinux")
        .timeout(Duration::from_secs(30))
        .call()
        .map_err(|e| format!("catalogue de configurations : {e}"))?;
    let body = response
        .into_string()
        .map_err(|e| format!("catalogue de configurations : {e}"))?;
    let catalog: Catalog =
        serde_json::from_str(&body).map_err(|e| format!("catalogue de configurations : {e}"))?;
    let catalog = configurations::merged(catalog);
    for entry in &catalog.configurations {
        let problems = entry.validate();
        if !problems.is_empty() {
            tracing::warn!(
                "configuration {} ignorée par l'interface : {}",
                entry.id,
                problems.join(" ; ")
            );
        }
    }
    // L'adresse retenue est celle du catalogue, sans le paramètre de cache :
    // elle sert ensuite à construire les URL des fichiers.
    Ok((catalog, configurations::catalog_url()))
}

/// Thèmes proposés : les thèmes déclaratifs encore publiés, puis les thèmes
/// natifs. Un thème natif n'est qu'un plugin `.so` marqué `theme` : le
/// catalogue n'a pas de seconde liste à tenir, et un thème ajouté au dépôt
/// apparaît comme les autres après la relecture.
fn theme_entries(catalog: &Catalog) -> Vec<ThemeEntry> {
    let mut entries = catalog.themes.clone();
    for plugin in catalog.plugins.iter().filter(|plugin| plugin.theme) {
        if entries.iter().any(|entry| entry.id == plugin.id) {
            continue;
        }
        let Some(binary) = plugin.binary.as_ref() else {
            continue;
        };
        entries.push(ThemeEntry {
            id: plugin.id.clone(),
            title: plugin.title.clone(),
            description: plugin.description.clone(),
            author: plugin.author.clone(),
            version: plugin.version.clone(),
            license: plugin.license.clone(),
            status: plugin.status.clone(),
            file: binary.file.clone(),
            sha256: binary.sha256.clone(),
            size: binary.size,
        });
    }
    entries
}

/// Thèmes installés : les manifestes de `themes/`, plus les thèmes natifs
/// posés dans `plugins/` — c'est là que le plugin natif se charge.
fn installed_theme_ids(themes_dir: &Path, plugins_dir: &Path, catalog: &Catalog) -> Vec<String> {
    let mut ids = installed_toml_ids(themes_dir);
    let plugins = installed_toml_ids(plugins_dir);
    for plugin in catalog.plugins.iter().filter(|plugin| plugin.theme) {
        if plugins.iter().any(|id| id == &plugin.id) && !ids.iter().any(|id| id == &plugin.id) {
            ids.push(plugin.id.clone());
        }
    }
    ids.sort();
    ids
}

impl super::Shared {
    /// Relit le catalogue si sa dernière lecture est ancienne. Le réseau ne
    /// bloque jamais une fiche : la réponse part en tâche de fond.
    pub fn start_configurations_in_background(self: &Arc<Self>) {
        {
            let Ok(mut state) = self.configurations.lock() else {
                return;
            };
            if state.running || !state.stale() {
                return;
            }
            state.running = true;
            state.last_try = lapinux_core::util::now_unix();
        }
        let force = self
            .configurations
            .lock()
            .map(|mut state| std::mem::take(&mut state.force))
            .unwrap_or(false);
        let shared = Arc::clone(self);
        std::thread::spawn(move || {
            match fetch(force) {
                Ok((catalog, url)) => {
                    let count = catalog.configurations.len();
                    if let Ok(mut state) = shared.configurations.lock() {
                        state.catalog = catalog;
                        state.remote = true;
                        state.fetched = lapinux_core::util::now_unix();
                        state.url = url;
                    }
                    tracing::info!("catalogue de configurations : {count} entrée(s)");
                }
                Err(err) => tracing::debug!("catalogue de configurations indisponible : {err}"),
            }
            if let Ok(mut state) = shared.configurations.lock() {
                state.running = false;
            }
        });
    }

    /// Force la prochaine lecture distante, même si la copie locale n'a pas
    /// encore expiré. Le bouton « Relire » doit voir une publication récente,
    /// pas attendre la fin du cache.
    pub fn refresh_configurations(self: &Arc<Self>) {
        if let Ok(mut state) = self.configurations.lock() {
            state.fetched = 0;
            state.last_try = 0;
            state.remote = false;
            state.force = true;
        }
        self.start_configurations_in_background();
    }

    /// Configurations connues pour un jeu, et celles déjà posées.
    pub fn game_configurations(&self, appid: u32) -> lapinux_core::Result<Envelope> {
        let game = self
            .store
            .game(appid)
            .map_err(|e| lapinux_core::Error::Other(e.to_string()))?;
        let (available, maintainer, remote, fetched, url) = {
            let state = self
                .configurations
                .lock()
                .map_err(|_| lapinux_core::Error::Other("catalogue indisponible".into()))?;
            let available = game
                .as_ref()
                .map(|game| state.for_game(game))
                .unwrap_or_default();
            (
                available,
                state.catalog.maintainer.clone(),
                state.remote,
                state.fetched,
                state.url.clone(),
            )
        };
        let installed = self.records()?.get(&appid).cloned().unwrap_or_default();
        Ok(Envelope::ok(&ConfigurationSet {
            configurations: available,
            installed,
            maintainer,
            remote,
            fetched,
            catalog_url: url,
        }))
    }

    /// Thèmes du catalogue et ceux déjà posés dans `themes/`. La lecture
    /// distante continue en tâche de fond : cette réponse ne l'attend pas.
    pub fn theme_catalog(self: &Arc<Self>) -> lapinux_core::Result<Envelope> {
        self.start_configurations_in_background();
        let (themes, remote, fetched, url) = {
            let state = self
                .configurations
                .lock()
                .map_err(|_| lapinux_core::Error::Other("catalogue indisponible".into()))?;
            (
                theme_entries(&state.catalog),
                state.remote,
                state.fetched,
                state.url.clone(),
            )
        };
        let catalog = self
            .configurations
            .lock()
            .map(|state| state.catalog.clone())
            .unwrap_or_default();
        Ok(Envelope::ok(&ThemeCatalogSet {
            themes,
            installed: installed_theme_ids(
                &self.paths.themes_dir(),
                &self.paths.plugins_dir(),
                &catalog,
            ),
            remote,
            fetched,
            catalog_url: url,
        }))
    }

    /// Plugins proposés par le catalogue et ceux déjà installés. Les binaires
    /// ne sont pas chargés ici : leur ABI est vérifiée au moment de l'installation.
    pub fn plugin_catalog(self: &Arc<Self>) -> lapinux_core::Result<Envelope> {
        self.start_configurations_in_background();
        let (plugins, palettes, remote, fetched, url) = {
            let state = self
                .configurations
                .lock()
                .map_err(|_| lapinux_core::Error::Other("catalogue indisponible".into()))?;
            (
                state.catalog.plugins.clone(),
                state.catalog.palettes.clone(),
                state.remote,
                state.fetched,
                state.url.clone(),
            )
        };
        let active = self.config_snapshot().general.active_plugins;
        Ok(Envelope::ok(&PluginCatalogSet {
            installed: installed_plugins(&self.paths.plugins_dir()),
            active,
            plugins,
            palettes,
            remote,
            fetched,
            catalog_url: url,
        }))
    }

    /// Plugins locaux réellement lisibles et leurs hooks, avec l'état
    /// d'activation demandé par la configuration courante.
    pub fn plugins(&self) -> lapinux_core::Result<Envelope> {
        let active = self.config_snapshot().general.active_plugins;
        Ok(Envelope::ok(&crate::plugins::installed(
            &self.paths,
            &active,
        )))
    }

    /// Installe un thème du catalogue : le fichier est téléchargé, son
    /// empreinte vérifiée, son manifeste contrôlé, puis écrit dans `themes/`.
    /// Aucun jeu n'est touché et rien ne s'active sans un choix de l'utilisateur.
    pub fn install_theme(&self, id: &str) -> lapinux_core::Result<Envelope> {
        // Un thème natif est publié comme un plugin `.so` marqué `theme` :
        // même téléchargement, même vérification, puis il devient le thème
        // actif. C'est ce qui permet d'ajouter un thème au dépôt sans mise à
        // jour de Lapinux.
        let native = {
            let state = self
                .configurations
                .lock()
                .map_err(|_| lapinux_core::Error::Other("catalogue indisponible".into()))?;
            state
                .catalog
                .plugins
                .iter()
                .any(|entry| entry.id == id && entry.theme)
        };
        if native {
            let installed = self.install_catalog_plugin(id)?;
            let mut config = self.config_snapshot();
            config.general.theme = id.to_string();
            self.save_config(config)?;
            return Ok(installed);
        }
        let (entry, base) = {
            let state = self
                .configurations
                .lock()
                .map_err(|_| lapinux_core::Error::Other("catalogue indisponible".into()))?;
            let entry = state
                .catalog
                .themes
                .iter()
                .find(|entry| entry.id == id)
                .cloned()
                .ok_or_else(|| lapinux_core::Error::Other("thème introuvable".into()))?;
            (entry, configurations::catalog_base(&state.url))
        };
        if !entry.available() {
            return Err(lapinux_core::Error::Other(
                "ce thème est signalé cassé par son auteur".into(),
            ));
        }
        let problems = entry.validate();
        if !problems.is_empty() {
            return Err(lapinux_core::Error::Other(format!(
                "thème invalide : {}",
                problems.join(" ; ")
            )));
        }
        let url = configurations::theme_url(&base, &entry)
            .ok_or_else(|| lapinux_core::Error::Other("thème sans adresse".into()))?;
        let bytes = download(&url, 256 * 1024).map_err(lapinux_core::Error::Other)?;
        let found = sha256_hex(&bytes);
        if !found.eq_ignore_ascii_case(entry.sha256.trim()) {
            return Err(lapinux_core::Error::Other(format!(
                "empreinte du thème incorrecte : attendu {}, obtenu {found}",
                entry.sha256
            )));
        }
        let target = write_theme(&self.paths.themes_dir(), &entry, &bytes)
            .map_err(lapinux_core::Error::Other)?;
        // Installer depuis le catalogue vaut activation : c'est le bouton
        // « Installer » de l'interface qui pilote ce chemin. Sans cela le
        // fichier existait mais l'interface restait sur l'ancien thème après
        // redémarrage.
        let mut config = self.config_snapshot();
        config.general.theme = entry.id.clone();
        self.save_config(config)?;
        Ok(Envelope::ok(&serde_json::json!({
            "id": entry.id,
            "path": target.display().to_string(),
            "active": true,
        })))
    }

    /// Installe un plugin local depuis un manifeste TOML. Le plugin n'a pas de
    /// catalogue : le fichier vient de l'utilisateur ou de l'éditeur, et il est
    /// rangé dans `plugins/` seulement après vérification de son identifiant.
    pub fn install_plugin(&self, manifest: &str, replace: bool) -> lapinux_core::Result<Envelope> {
        if manifest.len() > 256 * 1024 {
            return Err(lapinux_core::Error::Other(
                "manifeste de plugin trop gros".into(),
            ));
        }
        let value: toml::Value = toml::from_str(manifest)
            .map_err(|error| lapinux_core::Error::Other(format!("plugin illisible : {error}")))?;
        let id = value
            .get("id")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .trim();
        if id.is_empty()
            || id.contains('/')
            || id.contains('\\')
            || id.contains("..")
            || !id
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
        {
            return Err(lapinux_core::Error::Other(
                "identifiant de plugin invalide".into(),
            ));
        }
        let name = value
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .trim();
        if name.is_empty() {
            return Err(lapinux_core::Error::Other(
                "le plugin doit avoir un nom".into(),
            ));
        }
        let dir = self.paths.plugins_dir();
        std::fs::create_dir_all(&dir).map_err(|error| {
            lapinux_core::Error::Other(format!("dossier des plugins : {error}"))
        })?;
        let target = dir.join(format!("{id}.toml"));
        if target.exists() && !replace {
            return Err(lapinux_core::Error::Other(format!(
                "un plugin « {id} » existe déjà"
            )));
        }
        let temporary = dir.join(format!(".{id}.tmp"));
        std::fs::write(&temporary, manifest)
            .map_err(|error| lapinux_core::Error::Other(format!("écriture du plugin : {error}")))?;
        std::fs::rename(&temporary, &target)
            .map_err(|error| lapinux_core::Error::Other(format!("plugin installé : {error}")))?;
        let expected = sha256_hex(manifest.as_bytes());
        let written = std::fs::read(&target).map_err(|error| {
            lapinux_core::Error::Other(format!("relecture du plugin : {error}"))
        })?;
        if sha256_hex(&written) != expected {
            let _ = std::fs::remove_file(&target);
            return Err(lapinux_core::Error::Other(
                "le plugin écrit ne correspond pas au manifeste reçu".into(),
            ));
        }
        // Un plugin installé est immédiatement déclaré actif : le daemon
        // pourra charger ses hooks au prochain lancement de jeu, et l'UI le
        // retrouvera actif après redémarrage.
        let mut config = self.config_snapshot();
        if !config
            .general
            .active_plugins
            .iter()
            .any(|active| active == id)
        {
            config.general.active_plugins.push(id.to_string());
            self.save_config(config)?;
        }
        Ok(Envelope::ok(&serde_json::json!({
            "id": id,
            "path": target.display().to_string(),
            "sha256": expected,
            "active": true,
        })))
    }

    /// Installe un plugin du catalogue. Le manifeste et, pour un plugin natif,
    /// son binaire sont téléchargés et vérifiés avant toute écriture ; l'ABI
    /// est contrôlée sur le binaire temporaire.
    pub fn install_catalog_plugin(&self, id: &str) -> lapinux_core::Result<Envelope> {
        let (entry, base) = {
            let state = self
                .configurations
                .lock()
                .map_err(|_| lapinux_core::Error::Other("catalogue indisponible".into()))?;
            let entry = state
                .catalog
                .plugins
                .iter()
                .find(|entry| entry.id == id)
                .cloned()
                .ok_or_else(|| lapinux_core::Error::Other("plugin introuvable".into()))?;
            (entry, configurations::catalog_base(&state.url))
        };
        if !entry.available() {
            return Err(lapinux_core::Error::Other(
                "ce plugin est signalé cassé par son auteur".into(),
            ));
        }
        let problems = entry.validate();
        if !problems.is_empty() {
            return Err(lapinux_core::Error::Other(format!(
                "plugin invalide : {}",
                problems.join(" ; ")
            )));
        }
        check_compatibility(&entry).map_err(lapinux_core::Error::Other)?;
        // Un plugin du catalogue peut n'être qu'un .so : le manifeste local
        // est alors fabriqué à partir des métadonnées publiées, déjà vérifiées.
        let manifest = match configurations::plugin_manifest_url(&base, &entry) {
            Some(url) => {
                let bytes = download(&url, 256 * 1024).map_err(|error| {
                    lapinux_core::Error::Other(format!(
                        "manifeste manquant pour le plugin « {id} » : {error}"
                    ))
                })?;
                if bytes.len() as u64 != entry.size {
                    return Err(lapinux_core::Error::Other(format!(
                        "taille du manifeste de {id} incorrecte : attendu {}, obtenu {}",
                        entry.size,
                        bytes.len()
                    )));
                }
                let hash = sha256_hex(&bytes);
                if !hash.eq_ignore_ascii_case(entry.sha256.trim()) {
                    return Err(lapinux_core::Error::Other(format!(
                        "empreinte du manifeste de {id} incorrecte : attendu {}, obtenu {hash}",
                        entry.sha256
                    )));
                }
                bytes
            }
            None => plugin_manifest_from_catalog(&entry)?.into_bytes(),
        };
        let manifest_hash = sha256_hex(&manifest);
        let text = std::str::from_utf8(&manifest)
            .map_err(|_| lapinux_core::Error::Other("manifeste de plugin non UTF-8".into()))?;
        let native_library = plugin_manifest_library(text, id)?;
        let (binary, binary_hash) = if let Some(library) = native_library.as_deref() {
            let binary_entry = entry.binary.as_ref().ok_or_else(|| {
                lapinux_core::Error::Other(format!(
                    "le plugin « {id} » déclare une bibliothèque native mais aucun binaire n'est publié dans le catalogue"
                ))
            })?;
            let url = configurations::plugin_binary_url(&base, &entry).ok_or_else(|| {
                lapinux_core::Error::Other(format!("binaire manquant pour le plugin « {id} »"))
            })?;
            let bytes = download(&url, 64 * 1024 * 1024).map_err(|error| {
                lapinux_core::Error::Other(format!(
                    "binaire manquant pour le plugin « {id} » : {error}"
                ))
            })?;
            if bytes.len() as u64 != binary_entry.size {
                return Err(lapinux_core::Error::Other(format!(
                    "taille du binaire de {id} incorrecte : attendu {}, obtenu {}",
                    binary_entry.size,
                    bytes.len()
                )));
            }
            let hash = sha256_hex(&bytes);
            if !hash.eq_ignore_ascii_case(binary_entry.sha256.trim()) {
                return Err(lapinux_core::Error::Other(format!(
                    "empreinte du binaire de {id} incorrecte : attendu {}, obtenu {hash}",
                    binary_entry.sha256
                )));
            }
            if !library.ends_with(".so") {
                return Err(lapinux_core::Error::Other(format!(
                    "binaire de {id} invalide : une bibliothèque .so est attendue"
                )));
            }
            (Some((library.to_string(), bytes)), Some(hash))
        } else {
            if entry.binary.is_some() {
                return Err(lapinux_core::Error::Other(format!(
                    "binaire de {id} inattendu : le manifeste ne déclare aucune bibliothèque native"
                )));
            }
            (None, None)
        };
        let dir = self.paths.plugins_dir();
        std::fs::create_dir_all(&dir).map_err(|error| {
            lapinux_core::Error::Other(format!("dossier des plugins : {error}"))
        })?;
        let manifest_target = dir.join(format!("{id}.toml"));
        let manifest_tmp = dir.join(format!(".{id}.toml.tmp"));
        std::fs::write(&manifest_tmp, &manifest).map_err(|error| {
            lapinux_core::Error::Other(format!("écriture du manifeste de {id} : {error}"))
        })?;
        let mut binary_target = None;
        if let Some((library, bytes)) = binary {
            let target = dir.join(&library);
            let parent = target.parent().ok_or_else(|| {
                lapinux_core::Error::Other("binaire de plugin sans dossier".into())
            })?;
            std::fs::create_dir_all(parent).map_err(|error| {
                lapinux_core::Error::Other(format!("dossier du binaire de {id} : {error}"))
            })?;
            let temporary = parent.join(format!(".{id}.verify.so"));
            std::fs::write(&temporary, &bytes).map_err(|error| {
                let _ = std::fs::remove_file(&manifest_tmp);
                lapinux_core::Error::Other(format!("écriture du binaire de {id} : {error}"))
            })?;
            if let Err(error) = verify_plugin_abi(&temporary, id) {
                let _ = std::fs::remove_file(&temporary);
                let _ = std::fs::remove_file(&manifest_tmp);
                return Err(error);
            }
            std::fs::rename(&temporary, &target).map_err(|error| {
                let _ = std::fs::remove_file(&temporary);
                let _ = std::fs::remove_file(&manifest_tmp);
                lapinux_core::Error::Other(format!("binaire de {id} non installé : {error}"))
            })?;
            binary_target = Some(target);
        }
        std::fs::rename(&manifest_tmp, &manifest_target).map_err(|error| {
            let _ = std::fs::remove_file(&manifest_tmp);
            lapinux_core::Error::Other(format!("manifeste de {id} non installé : {error}"))
        })?;
        let mut config = self.config_snapshot();
        if !config
            .general
            .active_plugins
            .iter()
            .any(|active| active == id)
        {
            config.general.active_plugins.push(id.to_string());
            self.save_config(config)?;
        }
        Ok(Envelope::ok(&serde_json::json!({
            "id": id,
            "path": manifest_target.display().to_string(),
            "binary": binary_target.map(|path| path.display().to_string()),
            "manifest_sha256": manifest_hash,
            "binary_sha256": binary_hash,
            "active": true,
        })))
    }

    /// Installe une configuration sur un jeu. Tout est vérifié et sauvegardé
    /// avant la moindre écriture ; un échec remet le jeu dans son état.
    pub fn install_configuration(&self, appid: u32, id: &str) -> lapinux_core::Result<Envelope> {
        let _guard = self
            .save_operation
            .try_lock()
            .map_err(|_| lapinux_core::Error::Other("une opération est déjà en cours".into()))?;
        let game = self
            .store
            .game(appid)
            .map_err(|e| lapinux_core::Error::Other(e.to_string()))?
            .ok_or_else(|| lapinux_core::Error::Other("jeu introuvable".into()))?;
        if !game.installed {
            return Err(lapinux_core::Error::Other(
                "le jeu n'est pas installé : il n'y a rien à configurer".into(),
            ));
        }
        if self
            .session_info()
            .map(|session| session.appid == appid)
            .unwrap_or(false)
        {
            return Err(lapinux_core::Error::Other(
                "le jeu est en cours d'exécution : ferme-le avant d'installer une configuration"
                    .into(),
            ));
        }

        let (entry, base, source) = {
            let state = self
                .configurations
                .lock()
                .map_err(|_| lapinux_core::Error::Other("catalogue indisponible".into()))?;
            let entry = state
                .catalog
                .configurations
                .iter()
                .find(|entry| entry.id == id)
                .cloned()
                .ok_or_else(|| lapinux_core::Error::Other("configuration introuvable".into()))?;
            (
                entry,
                configurations::catalog_base(&state.url),
                state.url.clone(),
            )
        };
        if !entry.applies_to(&game) {
            return Err(lapinux_core::Error::Other(
                "cette configuration ne concerne pas ce jeu".into(),
            ));
        }
        let problems = entry.validate();
        if !problems.is_empty() {
            return Err(lapinux_core::Error::Other(format!(
                "configuration invalide : {}",
                problems.join(" ; ")
            )));
        }
        if entry.status == "broken" {
            return Err(lapinux_core::Error::Other(
                "cette configuration est signalée cassée par son auteur".into(),
            ));
        }

        let mut records = self.records()?;
        let previous = records
            .get(&appid)
            .and_then(|list| list.iter().find(|record| record.id == entry.id))
            .cloned();
        if entry.retired && previous.is_none() {
            return Err(lapinux_core::Error::Other(
                "cette configuration a été retirée du catalogue".into(),
            ));
        }
        let installed_ids: Vec<String> = records
            .get(&appid)
            .map(|list| list.iter().map(|record| record.id.clone()).collect())
            .unwrap_or_default();
        if let Some(conflict) = entry
            .conflicts
            .iter()
            .find(|id| installed_ids.iter().any(|installed| installed == *id))
        {
            return Err(lapinux_core::Error::Other(format!(
                "entre en conflit avec la configuration {conflict}"
            )));
        }
        if let Some(missing) = entry
            .requires
            .iter()
            .find(|id| !installed_ids.iter().any(|installed| installed == *id))
        {
            return Err(lapinux_core::Error::Other(format!(
                "demande d'abord la configuration {missing}"
            )));
        }

        // Tout est téléchargé et vérifié avant d'écrire : un catalogue
        // modifié ou un fichier tronqué n'atteint jamais le jeu.
        let mut payload: Vec<(FileSpec, Vec<u8>)> = Vec::new();
        let mut total = 0u64;
        for file in &entry.actions.files {
            let url = configurations::file_url(&base, &entry, file).ok_or_else(|| {
                lapinux_core::Error::Other(format!(
                    "adresse du fichier {} introuvable (catalogue sans dossier de base)",
                    file.source
                ))
            })?;
            let bytes = download(&url, MAX_FILE).map_err(lapinux_core::Error::Other)?;
            let hash = sha256_hex(&bytes);
            if !hash.eq_ignore_ascii_case(file.sha256.trim()) {
                return Err(lapinux_core::Error::Other(format!(
                    "empreinte incorrecte pour {} : le fichier publié ne correspond pas au catalogue",
                    file.source
                )));
            }
            total += bytes.len() as u64;
            if total > MAX_TOTAL {
                return Err(lapinux_core::Error::Other(
                    "configuration trop lourde (plus de 100 Mo)".into(),
                ));
            }
            payload.push((file.clone(), bytes));
        }

        let root = game.install_dir();
        if !root.is_dir() {
            return Err(lapinux_core::Error::Other(
                "dossier du jeu introuvable sur le disque".into(),
            ));
        }
        // Les injecteurs autonomes possèdent leurs DLL, même après une interruption.
        let injectors = self
            .graphics_injectors(appid)?
            .into_data::<Vec<lapinux_core::configurations::GraphicsInjector>>()?;
        for (file, _) in &payload {
            if injectors.iter().any(|injector| {
                let parent = Path::new(&injector.executable)
                    .parent()
                    .unwrap_or(Path::new(""));
                injector.files.iter().any(|owned| {
                    parent
                        .join(&owned.name)
                        .to_string_lossy()
                        .eq_ignore_ascii_case(&file.target)
                }) || Path::new(&file.target).starts_with(&injector.stage)
            }) {
                return Err(lapinux_core::Error::Other(format!(
                    "fichier suivi par un injecteur graphique : {}",
                    file.target
                )));
            }
        }
        // Une autre configuration ne doit pas prendre possession du même fichier.
        if let Some(other) = records
            .get(&appid)
            .into_iter()
            .flatten()
            .filter(|record| record.id != entry.id)
            .find(|record| {
                record.files.iter().any(|old| {
                    payload
                        .iter()
                        .any(|(file, _)| file.target.eq_ignore_ascii_case(&old.target))
                })
            })
        {
            return Err(lapinux_core::Error::Other(format!(
                "fichier déjà suivi par la configuration {}",
                other.id
            )));
        }
        if let Some(old) = &previous {
            // Un fichier modifié appartient au joueur. Une mise à jour ne doit ni
            // l'écraser ni perdre sa sauvegarde quand la liste de fichiers change.
            for file in &old.files {
                if !payload.iter().any(|(new, _)| new.target == file.target) {
                    return Err(lapinux_core::Error::Other(format!("mise à jour impossible : retirer d'abord la configuration pour préserver {}", file.target)));
                }
                let target =
                    checked_target(&root, &file.target).map_err(lapinux_core::Error::Other)?;
                if !target.is_file()
                    || !sha256_file(&target)
                        .map_err(lapinux_core::Error::Other)?
                        .eq_ignore_ascii_case(&file.sha256)
                {
                    return Err(lapinux_core::Error::Other(format!(
                        "{} a été modifié ou supprimé : mise à jour refusée",
                        file.target
                    )));
                }
            }
        }
        let backup_root = self
            .paths
            .data_dir
            .join("configurations")
            .join(appid.to_string())
            .join(&entry.id)
            .join("backup");
        let txn = self
            .paths
            .cache_dir
            .join("configuration-install")
            .join(format!("{appid}-{}", entry.id));
        let _ = std::fs::remove_dir_all(&txn);

        let mut written: Vec<InstalledFile> = Vec::new();
        for (file, bytes) in &payload {
            let created_by_previous = previous
                .as_ref()
                .and_then(|record| record.files.iter().find(|old| old.target == file.target))
                .map(|old| !old.backed_up)
                .unwrap_or(false);
            match write_file(&root, &backup_root, &txn, file, bytes, created_by_previous) {
                Ok(outcome) => written.push(outcome),
                Err(err) => {
                    rollback(&root, &txn, &written);
                    let _ = std::fs::remove_dir_all(&txn);
                    return Err(lapinux_core::Error::Other(err));
                }
            }
        }

        // Les réglages : ceux de l'ancienne version sont retirés d'abord pour
        // que la note garde les valeurs d'origine, pas celles du correctif
        // précédent.
        let mut settings = game.settings.clone();
        if let Some(old) = &previous {
            configurations::undo_settings(&mut settings, &old.undo);
        }
        let undo = entry.actions.settings.apply(&mut settings);
        let mut updated = game.clone();
        updated.settings = settings;
        if let Err(err) = self.store.save_game(&updated) {
            rollback(&root, &txn, &written);
            let _ = std::fs::remove_dir_all(&txn);
            return Err(lapinux_core::Error::Other(format!(
                "réglages non enregistrés : {err}"
            )));
        }

        let record = InstalledConfiguration {
            id: entry.id.clone(),
            title: entry.title.clone(),
            version: entry.version.clone(),
            installed_at: lapinux_core::util::now_unix(),
            files: written.clone(),
            undo,
            source,
        };
        let list = records.entry(appid).or_default();
        list.retain(|old| old.id != record.id);
        list.push(record.clone());
        if let Err(err) = self.save_records(&records) {
            // Les réglages sont déjà en place : on les retire pour ne pas
            // laisser une configuration impossible à désinstaller.
            // Rétablir exactement la fiche précédente, notamment lors d'une
            // mise à jour où elle portait encore l'ancienne configuration.
            let _ = self.store.save_game(&game);
            rollback(&root, &txn, &written);
            let _ = std::fs::remove_dir_all(&txn);
            return Err(lapinux_core::Error::Other(format!(
                "configuration non enregistrée : {err}"
            )));
        }

        // Les fichiers de l'ancienne version absents de la nouvelle ne sont
        // plus suivis : ils partent s'ils n'ont pas été modifiés.
        let mut warnings = Vec::new();
        if let Some(old) = &previous {
            let targets: Vec<&str> = written.iter().map(|file| file.target.as_str()).collect();
            for file in &old.files {
                if targets.contains(&file.target.as_str()) {
                    continue;
                }
                match remove_installed_file(&root, &backup_root, file) {
                    Ok(None) => {}
                    Ok(Some(warning)) => warnings.push(warning),
                    Err(err) => warnings.push(err),
                }
            }
        }
        let _ = std::fs::remove_dir_all(&txn);

        Ok(Envelope::ok(&InstallReport {
            id: entry.id,
            title: entry.title,
            version: entry.version,
            files: written.len(),
            bytes: total,
            warnings,
        }))
    }

    /// Retire une configuration et remet le jeu dans l'état d'avant. Un
    /// fichier modifié par le joueur depuis l'installation est conservé.
    pub fn remove_configuration(&self, appid: u32, id: &str) -> lapinux_core::Result<Envelope> {
        let _guard = self
            .save_operation
            .try_lock()
            .map_err(|_| lapinux_core::Error::Other("une opération est déjà en cours".into()))?;
        let game = self
            .store
            .game(appid)
            .map_err(|e| lapinux_core::Error::Other(e.to_string()))?
            .ok_or_else(|| lapinux_core::Error::Other("jeu introuvable".into()))?;
        if self
            .session_info()
            .map(|session| session.appid == appid)
            .unwrap_or(false)
        {
            return Err(lapinux_core::Error::Other(
                "le jeu est en cours d'exécution : ferme-le avant de retirer une configuration"
                    .into(),
            ));
        }
        let mut records = self.records()?;
        let record = records
            .get(&appid)
            .and_then(|list| list.iter().find(|record| record.id == id))
            .cloned()
            .ok_or_else(|| lapinux_core::Error::Other("configuration non installée".into()))?;

        let root = game.install_dir();
        let backup_root = self
            .paths
            .data_dir
            .join("configurations")
            .join(appid.to_string())
            .join(&record.id)
            .join("backup");
        let mut warnings = Vec::new();
        let mut unresolved = Vec::new();
        let mut removed = 0usize;
        for file in record.files.iter().rev() {
            match remove_installed_file(&root, &backup_root, file) {
                Ok(Some(warning)) => {
                    warnings.push(warning);
                    unresolved.push(file.clone());
                }
                Ok(None) => removed += 1,
                Err(err) => {
                    warnings.push(err);
                    unresolved.push(file.clone());
                }
            }
        }

        let mut settings = game.settings.clone();
        configurations::undo_settings(&mut settings, &record.undo);
        let mut updated = game.clone();
        updated.settings = settings;
        self.store
            .save_game(&updated)
            .map_err(|e| lapinux_core::Error::Other(format!("réglages non enregistrés : {e}")))?;

        let list = records.entry(appid).or_default();
        list.retain(|existing| existing.id != record.id);
        if !unresolved.is_empty() {
            let mut remaining = record.clone();
            remaining.files = unresolved;
            // Les réglages ont déjà été restaurés ; un second retrait ne doit
            // pas les restaurer une nouvelle fois.
            remaining.undo = Default::default();
            list.push(remaining);
        }
        self.save_records(&records)?;
        if warnings.is_empty() {
            let _ = std::fs::remove_dir_all(&backup_root);
        }

        let bytes = record
            .files
            .iter()
            .filter_map(|file| {
                std::fs::metadata(root.join(&file.target))
                    .ok()
                    .map(|meta| meta.len())
            })
            .sum();
        Ok(Envelope::ok(&InstallReport {
            id: record.id,
            title: record.title,
            version: record.version,
            files: removed,
            bytes,
            warnings,
        }))
    }

    pub(crate) fn graphics_files_conflict_with_configurations(
        &self,
        appid: u32,
        targets: &[String],
    ) -> lapinux_core::Result<bool> {
        let records = self.records()?;
        Ok(records
            .get(&appid)
            .into_iter()
            .flatten()
            .flat_map(|record| &record.files)
            .any(|file| {
                targets
                    .iter()
                    .any(|target| file.target.eq_ignore_ascii_case(target))
            }))
    }

    fn records(&self) -> lapinux_core::Result<HashMap<u32, Vec<InstalledConfiguration>>> {
        Ok(self
            .store
            .get_json::<HashMap<u32, Vec<InstalledConfiguration>>>(RECORDS)?
            .unwrap_or_default())
    }

    fn save_records(
        &self,
        records: &HashMap<u32, Vec<InstalledConfiguration>>,
    ) -> lapinux_core::Result<()> {
        self.store.set_json(RECORDS, records)?;
        Ok(())
    }
}

/// Télécharge un fichier en refusant tout ce qui dépasse le plafond.
/// Identifiants des plugins déjà présents : un fichier `<id>.toml` suffit.
fn installed_plugins(dir: &Path) -> Vec<String> {
    installed_toml_ids(dir)
}

fn installed_toml_ids(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
                return None;
            }
            // Un manifeste dont la bibliothèque native a disparu ne compte
            // pas : le plugin ne se chargerait pas, et l'interface doit
            // proposer de le réinstaller au lieu de le croire en place.
            if let Some(library) = declared_library(&path) {
                if !dir.join(library).is_file() {
                    return None;
                }
            }
            path.file_stem()?.to_str().map(str::to_string)
        })
        .collect();
    ids.sort();
    ids
}

/// Bibliothèque native déclarée par un manifeste, quand il en déclare une.
fn declared_library(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: toml::Value = toml::from_str(&text).ok()?;
    let library = value.get("native")?.get("library")?.as_str()?.trim();
    (!library.is_empty()).then(|| library.to_string())
}

/// Valide le manifeste minimal d'un plugin et renvoie sa bibliothèque native.
/// Refuse tôt un binaire qui ne peut pas se charger ici : architecture ou ABI
/// incompatible, ou Lapinux trop ancien. Un refus clair vaut mieux qu'un
/// téléchargement suivi d'un `dlopen` qui échoue.
fn check_compatibility(entry: &PluginEntry) -> Result<(), String> {
    if let Some(abi) = entry.abi {
        if abi != HOST_ABI_VERSION {
            return Err(format!(
                "le plugin « {} » demande l'ABI {abi} ; Lapinux utilise l'ABI {HOST_ABI_VERSION}",
                entry.id
            ));
        }
    }
    if let Some(arch) = entry.arch.as_deref() {
        let current = std::env::consts::ARCH;
        if arch != "any" && !arch.eq_ignore_ascii_case(current) {
            return Err(format!(
                "le plugin « {} » est publié pour {arch} ; cette machine est en {current}",
                entry.id
            ));
        }
    }
    if let Some(minimum) = entry.min_lapinux_version.as_deref() {
        if version_older(lapinux_core::VERSION, minimum) {
            return Err(format!(
                "le plugin « {} » demande Lapinux {minimum} ou plus récent (version installée : {})",
                entry.id,
                lapinux_core::VERSION
            ));
        }
    }
    Ok(())
}

fn version_parts(text: &str) -> Vec<u32> {
    text.trim()
        .split('.')
        .filter_map(|part| part.parse().ok())
        .collect()
}

fn version_older(current: &str, minimum: &str) -> bool {
    version_parts(current) < version_parts(minimum)
}

/// Fabrique le manifeste local d'un plugin publié sans manifeste : le
/// catalogue porte déjà l'identité et le binaire, la bibliothèque est copiée
/// à côté de ce fichier.
fn plugin_manifest_from_catalog(
    entry: &lapinux_core::configurations::PluginEntry,
) -> lapinux_core::Result<String> {
    let binary = entry.binary.as_ref().ok_or_else(|| {
        lapinux_core::Error::Other(format!(
            "le plugin « {} » n'a ni manifeste ni binaire",
            entry.id
        ))
    })?;
    let library = binary
        .file
        .rsplit('/')
        .next()
        .filter(|name| name.ends_with(".so"))
        .ok_or_else(|| {
            lapinux_core::Error::Other(format!(
                "binaire de {} invalide : une bibliothèque .so est attendue",
                entry.id
            ))
        })?;
    let mut root = toml::value::Table::new();
    root.insert("id".into(), toml::Value::String(entry.id.clone()));
    root.insert("name".into(), toml::Value::String(entry.title.clone()));
    root.insert(
        "description".into(),
        toml::Value::String(entry.description.clone()),
    );
    root.insert("version".into(), toml::Value::String(entry.version.clone()));
    root.insert("license".into(), toml::Value::String(entry.license.clone()));
    root.insert("status".into(), toml::Value::String(entry.status.clone()));
    let mut native = toml::value::Table::new();
    native.insert("library".into(), toml::Value::String(library.to_string()));
    native.insert("theme".into(), toml::Value::Boolean(entry.theme));
    root.insert("native".into(), toml::Value::Table(native));
    toml::to_string(&toml::Value::Table(root))
        .map_err(|error| lapinux_core::Error::Other(format!("manifeste de plugin : {error}")))
}

fn plugin_manifest_library(text: &str, expected_id: &str) -> lapinux_core::Result<Option<String>> {
    let value: toml::Value = toml::from_str(text)
        .map_err(|error| lapinux_core::Error::Other(format!("plugin illisible : {error}")))?;
    let id = value
        .get("id")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if !id.eq_ignore_ascii_case(expected_id) {
        return Err(lapinux_core::Error::Other(format!(
            "plugin {expected_id} : identifiant « {id} » inattendu"
        )));
    }
    let name = value
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if name.trim().is_empty() {
        return Err(lapinux_core::Error::Other(
            "le plugin doit avoir un nom".into(),
        ));
    }
    let Some(native) = value.get("native") else {
        return Ok(None);
    };
    let library = native
        .get("library")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .trim();
    if library.is_empty()
        || configurations::relative_path(library).is_err()
        || !library.ends_with(".so")
    {
        return Err(lapinux_core::Error::Other(
            "bibliothèque native de plugin invalide".into(),
        ));
    }
    Ok(Some(library.to_string()))
}

/// Charge le binaire temporaire sans appeler son initialisation et vérifie
/// qu'il parle bien l'ABI de l'hôte. Le descripteur reste valide tant que la
/// bibliothèque est vivante.
fn verify_plugin_abi(path: &Path, id: &str) -> lapinux_core::Result<()> {
    let library = unsafe { Library::new(path) }.map_err(|error| {
        lapinux_core::Error::Other(format!("binaire de {id} illisible : {error}"))
    })?;
    let entry: Symbol<'_, NativePluginEntry> = unsafe { library.get(PLUGIN_ENTRY_SYMBOL) }
        .map_err(|error| {
            lapinux_core::Error::Other(format!("entrée de plugin absente dans {id} : {error}"))
        })?;
    let descriptor = unsafe { entry() };
    if descriptor.is_null() {
        return Err(lapinux_core::Error::Other(format!(
            "descripteur de plugin nul pour {id}"
        )));
    }
    let abi = unsafe { (*descriptor).abi_version };
    if abi != HOST_ABI_VERSION {
        return Err(lapinux_core::Error::Other(format!(
            "ABI de {id} incompatible : binaire {abi}, hôte {HOST_ABI_VERSION}"
        )));
    }
    Ok(())
}

/// Valide un manifeste reçu puis l'écrit dans `themes/` par un temporaire
/// renommé. Le fichier n'est jamais partiel, et un thème invalide ne remplace
/// pas une version déjà installée.
fn write_theme(dir: &Path, entry: &ThemeEntry, bytes: &[u8]) -> Result<PathBuf, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "thème non UTF-8".to_string())?;
    configurations::validate_theme_manifest(text, &entry.id)?;
    std::fs::create_dir_all(dir).map_err(|error| format!("dossier des thèmes : {error}"))?;
    let target = dir.join(format!("{}.toml", entry.id));
    let temporary = dir.join(format!(".{}.tmp", entry.id));
    std::fs::write(&temporary, bytes).map_err(|error| format!("écriture du thème : {error}"))?;
    std::fs::rename(&temporary, &target).map_err(|error| format!("thème reçu : {error}"))?;
    Ok(target)
}

fn download(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let response = ureq::get(url)
        .set("User-Agent", "Lapinux")
        .timeout(Duration::from_secs(60))
        .call()
        .map_err(|e| format!("téléchargement de {url} : {e}"))?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("téléchargement de {url} : {e}"))?;
    if bytes.len() as u64 > limit {
        return Err(format!("fichier trop gros (plus de {limit} octets)"));
    }
    Ok(bytes)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut hasher = Sha256::new();
    let mut file = std::fs::File::open(path).map_err(|e| format!("{} : {e}", path.display()))?;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| format!("{} : {e}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Cible vérifiée à l'intérieur du dossier du jeu : les liens symboliques qui
/// en sortent sont refusés, comme les chemins absolus et les `..`.
fn checked_target(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let rel = configurations::relative_path(relative)?;
    let root = root
        .canonicalize()
        .map_err(|e| format!("dossier du jeu : {e}"))?;
    let mut target = root.clone();
    for component in rel.components() {
        target.push(component);
        if let Ok(meta) = std::fs::symlink_metadata(&target) {
            // Même un lien restant dans le jeu peut masquer une cible déjà
            // suivie ou changer de destination entre sauvegarde et retrait.
            if meta.file_type().is_symlink() {
                return Err(format!(
                    "{relative} traverse un lien symbolique : installation refusée"
                ));
            }
        }
    }
    let parent = target
        .parent()
        .ok_or_else(|| format!("{relative} n'a pas de dossier parent"))?;
    let mut existing = parent.to_path_buf();
    while !existing.exists() {
        existing.pop();
    }
    let real = existing
        .canonicalize()
        .map_err(|e| format!("dossier du jeu : {e}"))?;
    if !real.starts_with(&root) {
        return Err(format!("{relative} sort du dossier du jeu"));
    }
    Ok(target)
}

/// Écrit un fichier en gardant de quoi revenir en arrière : la version
/// courante part dans la transaction, l'original du jeu dans les sauvegardes.
fn write_file(
    root: &Path,
    backup_root: &Path,
    txn: &Path,
    file: &FileSpec,
    bytes: &[u8],
    created_by_previous: bool,
) -> Result<InstalledFile, String> {
    let relative = configurations::relative_path(&file.target)?;
    let target = checked_target(root, &file.target)?;
    let existed = target.exists();
    if existed {
        let saved = txn.join(&relative);
        if let Some(parent) = saved.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{} : {e}", parent.display()))?;
        }
        std::fs::copy(&target, &saved)
            .map_err(|e| format!("sauvegarde de {} avant écriture : {e}", relative.display()))?;
    }
    let mut backed_up = false;
    if existed && !created_by_previous {
        let backup = backup_root.join(&relative);
        if !backup.exists() {
            if let Some(parent) = backup.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("{} : {e}", parent.display()))?;
            }
            std::fs::copy(&target, &backup)
                .map_err(|e| format!("sauvegarde de {} : {e}", relative.display()))?;
        }
        backed_up = true;
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{} : {e}", parent.display()))?;
    }
    let temporary = target.with_extension(format!(
        "lapinux-{}-{}.tmp",
        std::process::id(),
        lapinux_core::util::now_unix()
    ));
    std::fs::write(&temporary, bytes)
        .map_err(|e| format!("écriture de {} : {e}", relative.display()))?;
    std::fs::rename(&temporary, &target).map_err(|e| {
        let _ = std::fs::remove_file(&temporary);
        format!("installation de {} : {e}", relative.display())
    })?;
    Ok(InstalledFile {
        target: file.target.clone(),
        sha256: sha256_hex(bytes),
        backed_up,
    })
}

/// Retire un fichier posé : restaure la sauvegarde s'il y en a une, efface
/// sinon. Un fichier modifié depuis l'installation est conservé et signalé.
fn remove_installed_file(
    root: &Path,
    backup_root: &Path,
    file: &InstalledFile,
) -> Result<Option<String>, String> {
    let target = checked_target(root, &file.target)?;
    if !target.exists() {
        if file.backed_up {
            return Ok(Some(format!(
                "{} est absent : sauvegarde conservée",
                file.target
            )));
        }
        return Ok(None);
    }
    let current = sha256_file(&target)?;
    if !current.eq_ignore_ascii_case(&file.sha256) {
        return Ok(Some(format!(
            "{} a été modifié depuis l'installation : conservé",
            file.target
        )));
    }
    let relative = configurations::relative_path(&file.target)?;
    let backup = backup_root.join(&relative);
    if file.backed_up && backup.exists() {
        std::fs::copy(&backup, &target)
            .map_err(|e| format!("restauration de {} : {e}", file.target))?;
        return Ok(None);
    }
    if file.backed_up {
        return Ok(Some(format!(
            "{} : la sauvegarde manque, le fichier est conservé",
            file.target
        )));
    }
    std::fs::remove_file(&target).map_err(|e| format!("retrait de {} : {e}", file.target))?;
    Ok(None)
}

/// Annule les écritures d'une installation qui a échoué.
fn rollback(root: &Path, txn: &Path, written: &[InstalledFile]) {
    for file in written.iter().rev() {
        let Ok(relative) = configurations::relative_path(&file.target) else {
            continue;
        };
        let Ok(target) = checked_target(root, &file.target) else {
            continue;
        };
        let saved = txn.join(&relative);
        if saved.exists() {
            if let Some(parent) = target.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::copy(&saved, &target);
        } else {
            let _ = std::fs::remove_file(&target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(target: &str, bytes: &[u8]) -> FileSpec {
        FileSpec {
            source: "files/test.pak".into(),
            target: target.into(),
            sha256: sha256_hex(bytes),
            backup: true,
        }
    }

    #[test]
    fn un_manifeste_de_plugin_natif_donne_sa_bibliotheque() {
        let text = r#"
id = "test-native"
name = "Test natif"
[native]
library = "liblapinux_plugin_test.so"
"#;
        assert_eq!(
            plugin_manifest_library(text, "test-native")
                .unwrap()
                .as_deref(),
            Some("liblapinux_plugin_test.so")
        );
        assert!(plugin_manifest_library(text, "autre").is_err());
        assert!(plugin_manifest_library(
            "id = \"x\"\nname = \"X\"\n[native]\nlibrary = \"../x.so\"\n",
            "x"
        )
        .is_err());
    }

    #[test]
    fn un_chemin_absolu_ou_remontant_est_refuse() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["../x", "/etc/passwd", "a/../../b"] {
            assert!(checked_target(dir.path(), bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn un_lien_symbolique_qui_sort_est_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("lien")).unwrap();
        assert!(checked_target(dir.path(), "lien/fichier.put").is_err());
    }

    #[test]
    fn un_lien_symbolique_interne_est_aussi_refuse() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("original"), b"x").unwrap();
        std::os::unix::fs::symlink("original", dir.path().join("alias")).unwrap();
        assert!(checked_target(dir.path(), "alias").is_err());
    }

    #[test]
    fn ecrire_then_restore() {
        let dir = tempfile::tempdir().unwrap();
        let backup = dir.path().join("backup");
        let txn = dir.path().join("txn");
        let game = dir.path().join("game");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("data.pak"), b"original").unwrap();

        let outcome = write_file(
            &game,
            &backup,
            &txn,
            &file("data.pak", b"nouveau"),
            b"nouveau",
            false,
        )
        .unwrap();
        assert!(outcome.backed_up);
        assert_eq!(std::fs::read(game.join("data.pak")).unwrap(), b"nouveau");

        let warning = remove_installed_file(&game, &backup, &outcome).unwrap();
        assert!(warning.is_none());
        assert_eq!(std::fs::read(game.join("data.pak")).unwrap(), b"original");
    }

    #[test]
    fn un_fichier_cree_est_supprime_au_retrait() {
        let dir = tempfile::tempdir().unwrap();
        let backup = dir.path().join("backup");
        let txn = dir.path().join("txn");
        let game = dir.path().join("game");
        std::fs::create_dir_all(&game).unwrap();
        let outcome =
            write_file(&game, &backup, &txn, &file("neuf.pak", b"x"), b"x", false).unwrap();
        assert!(!outcome.backed_up);
        assert!(game.join("neuf.pak").exists());
        remove_installed_file(&game, &backup, &outcome).unwrap();
        assert!(!game.join("neuf.pak").exists());
    }

    #[test]
    fn un_fichier_modifie_par_le_joueur_est_conserve() {
        let dir = tempfile::tempdir().unwrap();
        let backup = dir.path().join("backup");
        let txn = dir.path().join("txn");
        let game = dir.path().join("game");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("data.pak"), b"original").unwrap();
        let outcome = write_file(
            &game,
            &backup,
            &txn,
            &file("data.pak", b"config"),
            b"config",
            false,
        )
        .unwrap();
        std::fs::write(game.join("data.pak"), b"retouche").unwrap();
        let warning = remove_installed_file(&game, &backup, &outcome).unwrap();
        assert!(warning.is_some());
        assert_eq!(std::fs::read(game.join("data.pak")).unwrap(), b"retouche");
    }

    #[test]
    fn une_mise_a_jour_ne_remplace_pas_la_sauvegarde_d_origine() {
        let dir = tempfile::tempdir().unwrap();
        let backup = dir.path().join("backup");
        let txn = dir.path().join("txn");
        let game = dir.path().join("game");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("data.pak"), b"jeu").unwrap();
        let first =
            write_file(&game, &backup, &txn, &file("data.pak", b"v1"), b"v1", false).unwrap();
        assert!(first.backed_up);
        // La nouvelle version remplace le fichier, la sauvegarde reste celle
        // du jeu d'origine.
        let second =
            write_file(&game, &backup, &txn, &file("data.pak", b"v2"), b"v2", false).unwrap();
        assert!(second.backed_up);
        remove_installed_file(&game, &backup, &second).unwrap();
        assert_eq!(std::fs::read(game.join("data.pak")).unwrap(), b"jeu");
    }

    #[test]
    fn une_empreinte_ne_correspond_pas() {
        let bytes = b"contenu";
        assert_ne!(sha256_hex(bytes), sha256_hex(b"autre"));
        assert_eq!(sha256_hex(bytes).len(), 64);
    }

    #[test]
    fn un_plugin_sans_sa_bibliotheque_n_est_pas_installe() {
        let dir = std::env::temp_dir().join(format!("lapinux-plugins-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dossier des plugins");
        std::fs::write(
            dir.join("chat.toml"),
            "id = \"chat\"\n[native]\nlibrary = \"libchat.so\"\n",
        )
        .expect("manifeste");
        assert!(
            installed_plugins(&dir).is_empty(),
            "bibliothèque absente : rien d'installé"
        );
        std::fs::write(dir.join("libchat.so"), b"\x7fELF").expect("bibliothèque");
        assert_eq!(installed_plugins(&dir), vec!["chat".to_string()]);
        // Un manifeste sans bibliothèque native reste compté : rien à charger.
        std::fs::write(dir.join("simple.toml"), "id = \"simple\"\n").expect("manifeste");
        assert_eq!(
            installed_plugins(&dir),
            vec!["chat".to_string(), "simple".to_string()]
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn le_catalogue_compile_est_bien_forme() {
        let state = State::new();
        assert!(!state.catalog.configurations.is_empty());
        assert_eq!(state.catalog.maintainer.github, "PissenlitVert");
        // Aucun thème déclaratif n'est publié : NX est un plugin natif.
        assert!(state.catalog.themes.is_empty());
    }

    #[test]
    fn un_binaire_incompatible_est_refuse_avant_telechargement() {
        let mut entry = PluginEntry {
            id: "test".into(),
            abi: Some(HOST_ABI_VERSION),
            arch: Some(std::env::consts::ARCH.into()),
            min_lapinux_version: Some("0.1.0".into()),
            ..Default::default()
        };
        assert!(
            check_compatibility(&entry).is_ok(),
            "même machine, même ABI"
        );

        entry.abi = Some(HOST_ABI_VERSION + 1);
        assert!(check_compatibility(&entry).unwrap_err().contains("ABI"));

        entry.abi = None;
        entry.arch = Some("mips".into());
        assert!(check_compatibility(&entry).unwrap_err().contains("mips"));

        entry.arch = Some("any".into());
        entry.min_lapinux_version = Some("99.0.0".into());
        assert!(check_compatibility(&entry).unwrap_err().contains("99.0.0"));

        entry.min_lapinux_version = Some("0.0.1".into());
        assert!(check_compatibility(&entry).is_ok());
    }

    #[test]
    fn un_theme_natif_du_catalogue_est_un_theme() {
        let catalog: Catalog = serde_json::from_str(
            r#"{
                "version": 2,
                "plugins": [
                    {"id": "nx", "title": "NX", "description": "Carrousel.",
                     "author": {"name": "Pissenlit Vert"}, "version": "1.1.0",
                     "license": "CC0-1.0", "status": "experimental", "theme": true,
                     "binary": {"file": "themes/liblapinux_plugin_theme_nx.so",
                                "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                                "size": 506872}},
                    {"id": "chat", "title": "Chat", "description": "Pluie.",
                     "author": {"name": "La sieste"}, "version": "4.0.0",
                     "license": "CC0-1.0", "status": "experimental",
                     "binary": {"file": "plugins/liblapinux_plugin_chat.so",
                                "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                                "size": 425096}}
                ]
            }"#,
        )
        .expect("catalogue");
        let themes = theme_entries(&catalog);
        assert_eq!(themes.len(), 1, "un seul thème publié");
        assert_eq!(themes[0].id, "nx");
        assert_eq!(themes[0].file, "themes/liblapinux_plugin_theme_nx.so");
        assert_eq!(themes[0].size, 506872);
        assert!(themes[0].available());

        // Le thème natif s'installe dans `plugins/` : c'est là que le plugin
        // natif se charge, et la liste doit le compter comme installé.
        let root = std::env::temp_dir().join(format!("lapinux-themes-{}", std::process::id()));
        let themes_dir = root.join("themes");
        let plugins_dir = root.join("plugins");
        std::fs::create_dir_all(&themes_dir).expect("dossier des thèmes");
        std::fs::create_dir_all(&plugins_dir).expect("dossier des plugins");
        std::fs::write(plugins_dir.join("nx.toml"), "id = \"nx\"\n").expect("manifeste");
        let installed = installed_theme_ids(&themes_dir, &plugins_dir, &catalog);
        assert_eq!(installed, vec!["nx".to_string()]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn un_plugin_sans_manifeste_recoit_un_manifeste_local() {
        let entry: lapinux_core::configurations::PluginEntry =
            serde_json::from_value(serde_json::json!({
                "id": "chat",
                "title": "Miaou",
                "description": "Une pluie de miaou.",
                "version": "4.0.0",
                "license": "CC0-1.0",
                "status": "experimental",
                "theme": false,
                "binary": {
                    "file": "plugins/liblapinux_plugin_chat.so",
                    "sha256": "a".repeat(64),
                    "size": 1024
                }
            }))
            .expect("entrée de catalogue");
        assert!(entry.validate().is_empty(), "{:?}", entry.validate());
        let text = plugin_manifest_from_catalog(&entry).expect("manifeste fabriqué");
        assert_eq!(
            plugin_manifest_library(&text, "chat")
                .expect("manifeste lisible")
                .as_deref(),
            Some("liblapinux_plugin_chat.so")
        );
        assert!(text.contains("id = \"chat\""));
    }

    #[test]
    fn un_theme_du_catalogue_est_ecrit_apres_validation() {
        let dir = tempfile::tempdir().unwrap();
        let entry = ThemeEntry {
            id: "nx".into(),
            ..Default::default()
        };
        let text = "id = \"nx\"\nlayout = \"nx\"\naccent = \"#00C8E0\"\n";
        let target = write_theme(dir.path(), &entry, text.as_bytes()).unwrap();
        assert_eq!(target.file_name().unwrap(), "nx.toml");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), text);
        // Un manifeste dont l'identifiant ne correspond pas à l'entrée ne doit
        // rien remplacer.
        let other = ThemeEntry {
            id: "autre".into(),
            ..Default::default()
        };
        assert!(write_theme(dir.path(), &other, text.as_bytes()).is_err());
        assert!(!dir.path().join("autre.toml").exists());
    }
}
