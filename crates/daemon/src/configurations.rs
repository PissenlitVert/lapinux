//! Configurations de jeux : catalogue publié par Pissenlit Vert, installation
//! et retrait des fichiers et réglages qu'elles décrivent.
//!
//! Rien n'est exécuté : une configuration ne pose que des réglages et des
//! fichiers. Chaque fichier remplacé est sauvegardé, chaque variable touchée
//! garde sa valeur d'avant, et l'installation entière est annulée si une
//! étape échoue.

use lapinux_core::configurations::{
    self, Catalog, Configuration, ConfigurationSet, FileSpec, InstallReport,
    InstalledConfiguration, InstalledFile, ThemeCatalogSet, ThemeEntry,
};
use lapinux_core::ipc::Envelope;
use lapinux_core::model::Game;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Délai entre deux lectures du catalogue distant.
const MAX_AGE: i64 = 6 * 60 * 60;
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

/// Relit le catalogue publié.
fn fetch() -> Result<(Catalog, String), String> {
    let url = configurations::catalog_url();
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
    Ok((catalog, url))
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
        let shared = Arc::clone(self);
        std::thread::spawn(move || {
            match fetch() {
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
                state.catalog.themes.clone(),
                state.remote,
                state.fetched,
                state.url.clone(),
            )
        };
        Ok(Envelope::ok(&ThemeCatalogSet {
            themes,
            installed: installed_themes(&self.paths.themes_dir()),
            remote,
            fetched,
            catalog_url: url,
        }))
    }

    /// Installe un thème du catalogue : le fichier est téléchargé, son
    /// empreinte vérifiée, son manifeste contrôlé, puis écrit dans `themes/`.
    /// Aucun jeu n'est touché et rien ne s'active sans un choix de l'utilisateur.
    pub fn install_theme(&self, id: &str) -> lapinux_core::Result<Envelope> {
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
        Ok(Envelope::ok(&serde_json::json!({
            "id": entry.id,
            "path": target.display().to_string(),
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
/// Identifiants des thèmes déjà présents : le nom du fichier suffit, il est
/// écrit depuis l'identifiant validé du catalogue.
fn installed_themes(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            (path.extension().and_then(|ext| ext.to_str()) == Some("toml"))
                .then(|| path.file_stem()?.to_str().map(|stem| stem.to_string()))
                .flatten()
        })
        .collect();
    ids.sort();
    ids
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
    fn le_catalogue_compile_est_bien_forme() {
        let state = State::new();
        assert!(!state.catalog.configurations.is_empty());
        assert_eq!(state.catalog.maintainer.github, "PissenlitVert");
        // NX n'est plus compilé dans l'interface : il n'existe que par cette
        // entrée de catalogue.
        assert!(state.catalog.themes.iter().any(|theme| theme.id == "nx"));
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
