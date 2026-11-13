//! Catalogue, cache de fiches et succès Steam.

use super::*;

/// Relevé des appids du compte, un par compte : il sert de base au contrôle
/// léger du démarrage. Seuls les appids apparus depuis ce relevé sont ajoutés.
const ACCOUNT_APPIDS_KEY: &str = "steam.account_appids";
/// Date du dernier contrôle léger, un par compte. Redémarrer l'application
/// plusieurs fois de suite ne doit pas rouvrir une session Steam à chaque
/// lancement : c'est cette répétition qui déclenchait la limitation de débit.
const ACCOUNT_PROBE_KEY: &str = "steam.account_probe_at";
/// Un contrôle léger au plus toutes les trente minutes. Les achats restent
/// visibles au lancement suivant, sans marteler le compte pendant les essais.
const ACCOUNT_PROBE_INTERVAL: i64 = 30 * 60;
/// Le contrôle attend que l'interface soit ouverte et que les préchargements
/// aient fini leur premier passage : le démarrage lui-même reste sans requête.
const ACCOUNT_PROBE_DELAY: std::time::Duration = std::time::Duration::from_secs(180);

/// Appids apparus depuis le relevé précédent et encore absents du catalogue.
fn new_account_appids(previous: &HashSet<u32>, known: &HashSet<u32>, appids: &[u32]) -> Vec<u32> {
    appids
        .iter()
        .copied()
        .filter(|appid| !previous.contains(appid) && !known.contains(appid))
        .collect()
}

/// Reprend les fiches ajoutées par une clé : la relève complète ne les connaît
/// pas toujours (paquet sans fiche PICS, jeu non encore publié), mais leur date
/// est dans `stamped` et elles ne doivent pas disparaître du catalogue.
fn keep_stamped_entries(
    entries: &mut Vec<lapinux_steam::catalog::Entry>,
    previous: &lapinux_steam::catalog::Catalog,
) {
    let mut known: HashSet<u32> = entries.iter().map(|entry| entry.appid).collect();
    for old in &previous.entries {
        if previous.stamped.contains_key(&old.appid) && known.insert(old.appid) {
            entries.push(old.clone());
        }
    }
}

impl Shared {
    /// Catalogue des jeux possédés. `refresh` lance une synchronisation en
    /// arrière-plan (licences Steam, puis noms) et rend tout de suite le cache :
    /// une grosse bibliothèque ne bloque ni le daemon ni l'interface, qui sonde.
    pub fn steam_catalog(self: &Arc<Self>, refresh: bool) -> Result<Envelope> {
        let config = self.config_snapshot();
        let account = config.steam.username.trim().to_string();

        if refresh {
            if account.is_empty() {
                return Err(lapinux_core::Error::Other(
                    "connectez d'abord votre compte Steam : la bibliothèque en dépend".into(),
                ));
            }
            self.start_catalog_refresh(&account);
        }

        let catalog = lapinux_steam::catalog::Catalog::load(&self.catalog_path());
        // Un cache existant, même d'une version précédente, n'entraîne plus de
        // synchronisation au démarrage. Le bouton « Vérifier les nouveaux
        // jeux » et le contrôle léger du compte couvrent les nouveautés ; une
        // bibliothèque de 1 500 jeux ne doit pas rouvrir Steam à chaque
        // lancement.
        // Jeux installés et leur bibliothèque : c'est là que se trouvent les
        // manifestes, donc là que se lit l'état d'un contenu additionnel.
        let installed: std::collections::HashMap<u32, std::path::PathBuf> = self
            .store
            .load_games()
            .unwrap_or_default()
            .into_iter()
            .filter(|game| game.installed)
            .map(|game| (game.appid, game.library))
            .collect();
        let family_dlcs: Vec<lapinux_core::model::SteamFamilyDlc> = catalog
            .family_dlcs
            .iter()
            .map(|dlc| lapinux_core::model::SteamFamilyDlc {
                appid: dlc.appid,
                name: dlc.name.clone(),
                owners: dlc.owners.clone(),
            })
            .collect();
        let mut entries: Vec<lapinux_core::model::SteamCatalogGame> = catalog
            .entries
            .iter()
            .filter(|entry| entry.is_game() || entry.is_demo())
            .map(|entry| lapinux_core::model::SteamCatalogGame {
                appid: entry.appid,
                name: entry.display_name(),
                installed: installed.contains_key(&entry.appid),
                family: entry.family,
                details: entry.details.clone(),
                acquired: entry.acquired,
                // La raison est mesurée ici, une fois toutes les sources lues :
                // prêté, gratuit, ou simplement non publié par Steam.
                undated_reason: entry.acquired.is_none().then(|| {
                    if entry.family {
                        "family"
                    } else if entry.details.free {
                        "free"
                    } else {
                        "unpublished"
                    }
                    .to_string()
                }),
                minutes: entry.minutes,
                dlc: catalog
                    .dlcs_of(entry)
                    .into_iter()
                    .map(|dlc| lapinux_core::model::SteamDlc {
                        installed: installed
                            .get(&entry.appid)
                            .map(|library| extra_content_installed(library, dlc.appid))
                            .unwrap_or(false),
                        appid: dlc.appid,
                        name: dlc.name,
                        download: dlc.download,
                        install: dlc.install,
                    })
                    .collect(),
            })
            .collect();
        entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        Ok(Envelope::ok(&lapinux_core::model::SteamCatalog {
            account: catalog.account,
            updated: catalog.updated,
            entries,
            family_dlcs,
            syncing: self.catalog_running.load(Ordering::SeqCst),
            error: self.catalog_error.lock().ok().and_then(|slot| slot.clone()),
        }))
    }

    /// Une lecture par jeu, servie depuis le cache tant qu'elle est fraîche.
    ///
    /// `kind` distingue les lectures (fiche, succès), `produce` fait le vrai
    /// travail : une seule lecture à la fois par jeu, les autres recevant la
    /// valeur connue plutôt que d'ouvrir une session Steam de plus.
    fn cached_game_read<F>(
        &self,
        kind: &'static str,
        appid: u32,
        disk: Option<&Path>,
        produce: F,
    ) -> Result<Envelope>
    where
        F: FnOnce() -> Result<serde_json::Value>,
    {
        for _ in 0..50 {
            let (fresh, known) = {
                let mut cache = self
                    .game_reads
                    .lock()
                    .map_err(|_| lapinux_core::Error::Other("cache Steam indisponible".into()))?;
                let fresh = cache.is_fresh(kind, appid);
                (fresh, cache.any(kind, appid))
            };
            if let (true, Some(value)) = (fresh, known.as_ref()) {
                return Ok(Envelope::ok(value));
            }
            let started = {
                let mut cache = self
                    .game_reads
                    .lock()
                    .map_err(|_| lapinux_core::Error::Other("cache Steam indisponible".into()))?;
                if cache.running.contains(&(kind, appid)) {
                    false
                } else {
                    cache.begin(kind, appid)
                }
            };
            if started {
                let value = produce();
                let saved = value
                    .as_ref()
                    .ok()
                    .is_some_and(|value| disk.is_some_and(|path| Self::save_sheet(path, value)));
                let mut cache = self
                    .game_reads
                    .lock()
                    .map_err(|_| lapinux_core::Error::Other("cache Steam indisponible".into()))?;
                match &value {
                    Ok(value) => cache.finish(kind, appid, value.clone(), saved),
                    Err(_) => cache.abandon(kind, appid),
                }
                return value.map(|value| Envelope::ok(&value));
            }
            // Une lecture tourne déjà : sa valeur, même ancienne, vaut mieux
            // qu'une seconde session Steam pour le même jeu.
            if let Some(value) = known {
                return Ok(Envelope::ok(&value));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        produce().map(|value| {
            if let Some(path) = disk {
                Self::save_sheet(path, &value);
            }
            Envelope::ok(&value)
        })
    }

    /// Fiche complète d'un jeu, relue à chaque ouverture : le compte par la Web
    /// API, le protocole par PICS, la boutique par sa page publique. Chaque
    /// source est tentée séparément — une source muette laisse une note et
    /// n'empêche pas les autres d'être lues.
    /// La fiche connue du jeu part tout de suite, même périmée, et sa
    /// relecture réseau se fait derrière : ouvrir un jeu ne doit plus coûter
    /// plusieurs secondes de protocole à chaque fois, seulement la première.
    pub fn game_info(self: &Arc<Self>, appid: u32) -> Result<Envelope> {
        let shared = Arc::clone(self);
        self.cached_remote_read("info", appid, move || shared.read_game_info(appid))
    }

    /// Fichier d'une fiche gardée sur disque : un dossier stable pour Steam
    /// et un pour Epic, GOG et itch.
    fn sheet_path(&self, dir: &str, appid: u32) -> std::path::PathBuf {
        self.paths.cache_dir.join(dir).join(format!("{appid}.json"))
    }

    /// Fiche Steam, dans le dossier stable `fiches`.
    fn game_info_path(&self, appid: u32) -> std::path::PathBuf {
        self.sheet_path(STEAM_SHEET_DIR, appid)
    }

    /// Fichier de cache disque d'une lecture Steam. La fiche et les pages de
    /// boutique ont leur dossier historique ; les succès et le cloud gagnent
    /// le leur pour survivre à un redémarrage de l'interface.
    fn remote_sheet_path(&self, kind: &str, appid: u32) -> Option<std::path::PathBuf> {
        let dir = match kind {
            "info" => STEAM_SHEET_DIR.to_string(),
            "boutique" => STORE_SHEET_DIR.to_string(),
            // Les succès et le cloud sont propres au compte : un second
            // compte ne doit jamais voir le relevé du premier.
            "succès" => format!("succes/{}", self.cache_account()),
            "cloud" => format!("clouds/{}", self.cache_account()),
            _ => return None,
        };
        Some(self.sheet_path(&dir, appid))
    }

    /// Compte Steam sous lequel les lectures personnelles sont rangées.
    /// Un nom vide garde un dossier commun, sans exposer de chemin arbitraire.
    fn cache_account(&self) -> String {
        let account = self.config_snapshot().steam.username;
        let clean: String = account
            .chars()
            .filter(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
            })
            .collect();
        if clean.is_empty() {
            "local".into()
        } else {
            clean.to_ascii_lowercase()
        }
    }

    /// Supprime la copie disque d'une lecture qui vient de changer (cloud
    /// restauré, partie terminée…). La mémoire seule serait insuffisante : au
    /// prochain redémarrage, l'ancienne copie reviendrait.
    pub(crate) fn forget_remote_sheet(&self, kind: &str, appid: u32) {
        if let Some(path) = self.remote_sheet_path(kind, appid) {
            let _ = std::fs::remove_file(path);
        }
    }

    /// Fiche écrite par un passage précédent, si elle se relit.
    fn read_sheet(path: &Path) -> Option<serde_json::Value> {
        let bytes = std::fs::read(path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Garde une fiche sur disque : ouvrir un jeu juste après un redémarrage
    /// ne repaie plus la lecture réseau. L'écriture passe par un fichier
    /// temporaire pour qu'une fiche à moitié écrite ne soit jamais relue.
    pub(crate) fn save_sheet(path: &Path, value: &serde_json::Value) -> bool {
        let Some(dir) = path.parent() else {
            return false;
        };
        if std::fs::create_dir_all(dir).is_err() {
            return false;
        }
        let temp = path.with_extension("tmp");
        let Ok(bytes) = serde_json::to_vec(value) else {
            return false;
        };
        std::fs::write(&temp, bytes).is_ok() && std::fs::rename(&temp, path).is_ok()
    }

    /// Réponse connue servie tout de suite — mémoire, puis disque — et relecture
    /// réseau derrière quand elle a dépassé sa fenêtre. Les fiches, les succès
    /// et le cloud sont tenus sur disque ; les lectures personnelles sont
    /// rangées sous le compte Steam qui les a lues.
    pub(crate) fn cached_remote_read<F>(
        self: &Arc<Self>,
        kind: &'static str,
        appid: u32,
        produce: F,
    ) -> Result<Envelope>
    where
        F: Fn() -> Result<serde_json::Value> + Send + Sync + 'static,
    {
        let disk = self.remote_sheet_path(kind, appid);
        let (known, fresh) = {
            let mut cache = self
                .game_reads
                .lock()
                .map_err(|_| lapinux_core::Error::Other("cache Steam indisponible".into()))?;
            (cache.any(kind, appid), cache.is_fresh(kind, appid))
        };
        if let Some(known) = known {
            if !fresh {
                self.refresh_remote_read(kind, appid, Arc::new(produce), disk);
            }
            return Ok(Envelope::ok(&known));
        }
        // Rien en mémoire (premier lancement, ou jeu jamais ouvert depuis ce
        // démarrage) : la fiche écrite au dernier passage s'affiche aussitôt.
        if let Some(known) = disk.as_deref().and_then(Self::read_sheet) {
            if let Ok(mut cache) = self.game_reads.lock() {
                cache.seed(kind, appid, known.clone());
            }
            self.refresh_remote_read(kind, appid, Arc::new(produce), disk);
            return Ok(Envelope::ok(&known));
        }
        self.cached_game_read(kind, appid, disk.as_deref(), || produce())
    }

    /// Relit en tâche de fond, une seule fois à la fois : la réponse suivante
    /// sera fraîche sans que l'écran ait attendu.
    fn refresh_remote_read<F>(
        self: &Arc<Self>,
        kind: &'static str,
        appid: u32,
        produce: Arc<F>,
        disk: Option<std::path::PathBuf>,
    ) where
        F: Fn() -> Result<serde_json::Value> + Send + Sync + 'static,
    {
        let started = self
            .game_reads
            .lock()
            .map(|mut cache| cache.begin(kind, appid))
            .unwrap_or(false);
        if !started {
            return;
        }
        let shared = Arc::clone(self);
        std::thread::spawn(move || {
            let value = produce();
            let saved = value.as_ref().ok().is_some_and(|value| {
                disk.as_deref()
                    .is_some_and(|path| Self::save_sheet(path, value))
            });
            if let Ok(mut cache) = shared.game_reads.lock() {
                match &value {
                    Ok(value) => cache.finish(kind, appid, value.clone(), saved),
                    Err(_) => cache.abandon(kind, appid),
                }
            }
        });
    }

    /// Ménage du dossier de fiches : restes d'écriture, fiches trop vieilles,
    /// puis les plus anciennes au-delà du plafond. Appelé au démarrage ; sans
    /// lui, le dossier grossissait d'un fichier par jeu ouvert, sans fin.
    pub fn prune_game_info_cache(&self) {
        self.purge_legacy_sheet_caches();
        self.ensure_sheet_cache_dirs();
        let mut removed = 0usize;
        for dir in [STEAM_SHEET_DIR, STORE_SHEET_DIR] {
            removed += Self::prune_sheet_dir(&self.paths.cache_dir.join(dir));
        }
        if removed > 0 {
            tracing::debug!("fiches : {removed} fichier(s) de cache retiré(s)");
        }
    }

    /// Un seul dossier par famille de fiches. Le marqueur `.format` permet de
    /// périmer une ancienne structure sans multiplier les dossiers `v2`, `v3`…
    pub(crate) fn ensure_sheet_cache_dirs(&self) {
        let _ = std::fs::create_dir_all(&self.paths.cache_dir);
        for dir in [STEAM_SHEET_DIR, STORE_SHEET_DIR] {
            let path = self.paths.cache_dir.join(dir);
            let marker = self.paths.cache_dir.join(format!("{dir}.format"));
            let current = std::fs::read_to_string(&marker)
                .ok()
                .is_some_and(|value| value.trim() == SHEET_CACHE_FORMAT);
            if !current {
                Self::remove_cache_entry(&path);
                let _ = std::fs::write(&marker, SHEET_CACHE_FORMAT);
            }
            let _ = std::fs::create_dir_all(&path);
        }
    }

    /// Les dossiers versionnés des anciennes builds ne sont plus lus : ils
    /// partent au premier démarrage qui utilise les dossiers stables.
    fn purge_legacy_sheet_caches(&self) {
        for dir in LEGACY_SHEET_DIRS {
            Self::remove_cache_entry(&self.paths.cache_dir.join(dir));
        }
    }

    /// Retire une entrée de cache sans suivre un éventuel lien symbolique.
    fn remove_cache_entry(path: &Path) {
        let Ok(meta) = std::fs::symlink_metadata(path) else {
            return;
        };
        if meta.file_type().is_symlink() || meta.is_file() {
            let _ = std::fs::remove_file(path);
        } else {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    /// Vide les caches récupérables. Les installateurs de boutiques restent
    /// en place : une installation ou une reprise peut les utiliser à tout
    /// moment. Les anciennes jaquettes personnalisées restent aussi intactes.
    pub fn clear_caches(&self) -> Result<u64> {
        let mut dirs = vec![
            "Covers",
            "Art",
            "webview",
            STEAM_SHEET_DIR,
            STORE_SHEET_DIR,
            "succes",
            "clouds",
            "Thumbs",
            "Previews",
        ];
        dirs.extend(LEGACY_SHEET_DIRS.iter().copied());
        let mut targets: Vec<(PathBuf, bool)> = dirs
            .into_iter()
            .map(|dir| (self.paths.cache_dir.join(dir), dir == "Covers"))
            .collect();
        targets.push((self.paths.store_web_dir().join("cache"), false));

        // Un chemin personnalisé peut placer les données importantes sous un
        // cache. Refuser avant toute suppression si les chemins se recouvrent.
        for parent in [&self.paths.cache_dir, &self.paths.store_web_dir()] {
            match std::fs::symlink_metadata(parent) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(lapinux_core::Error::Other(format!(
                        "{} est un lien symbolique : vidage refusé",
                        parent.display()
                    )));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(lapinux_core::Error::Other(format!(
                        "{} : {error}",
                        parent.display()
                    )))
                }
            }
        }
        let config = self.config_snapshot();
        let protected = [
            self.paths.config_dir.clone(),
            config.library_root(),
            crate::saves::root(&self.paths, &config),
        ];
        for (target, _) in &targets {
            let Ok(target_real) = std::fs::canonicalize(target) else {
                continue;
            };
            for path in &protected {
                if let Ok(real) = std::fs::canonicalize(path) {
                    if real.starts_with(&target_real) || target_real.starts_with(&real) {
                        return Err(lapinux_core::Error::Other(format!(
                            "{} chevauche un dossier à conserver ({})",
                            target.display(),
                            path.display()
                        )));
                    }
                }
            }
            if let Ok(data) = std::fs::canonicalize(&self.paths.data_dir) {
                // Seul le cache WebKit est intentionnellement dans data_dir.
                let web_cache = self.paths.store_web_dir().join("cache");
                if data.starts_with(&target_real)
                    || (target_real.starts_with(&data) && *target != web_cache)
                {
                    return Err(lapinux_core::Error::Other(format!(
                        "{} chevauche les données Lapinux",
                        target.display()
                    )));
                }
            }
        }
        let mut freed = 0;
        for (dir, preserve_custom) in targets {
            freed += Self::remove_tree(&dir, preserve_custom).map_err(|error| {
                lapinux_core::Error::Other(format!("{} : {error}", dir.display()))
            })?;
        }
        if let Ok(mut cache) = self.game_reads.lock() {
            cache.clear();
        }
        self.ensure_sheet_cache_dirs();
        Ok(freed)
    }

    /// Oublie les fiches de boutique : description et annonces sont écrites
    /// dans la langue choisie, un changement de langue les périme.
    pub fn forget_store_sheets(&self) {
        if let Ok(mut cache) = self.game_reads.lock() {
            cache.forget_kind("boutique");
        }
        let _ = std::fs::remove_dir_all(self.paths.cache_dir.join(STORE_SHEET_DIR));
        self.ensure_sheet_cache_dirs();
    }

    /// Ne suit jamais un lien, y compris quand la racine est un lien.
    fn remove_tree(dir: &Path, preserve_custom: bool) -> std::io::Result<u64> {
        let meta = match std::fs::symlink_metadata(dir) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(error),
        };
        if !meta.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "le dossier de cache n'est pas un vrai répertoire",
            ));
        }
        let mut freed = 0;
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if preserve_custom && entry.file_name().to_string_lossy().starts_with("custom-") {
                continue;
            }
            let meta = std::fs::symlink_metadata(&path)?;
            if meta.is_dir() {
                freed += Self::remove_tree(&path, false)?;
                std::fs::remove_dir(&path)?;
            } else {
                std::fs::remove_file(&path)?;
                freed += meta.len();
            }
        }
        Ok(freed)
    }

    /// Ménage d'un dossier de fiches ; rend le nombre de fichiers retirés.
    fn prune_sheet_dir(dir: &Path) -> usize {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        let now = std::time::SystemTime::now();
        let mut kept: Vec<(std::time::SystemTime, std::path::PathBuf)> = Vec::new();
        let mut removed = 0usize;
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let written = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            let age = now.duration_since(written).unwrap_or_default();
            // Un `.tmp` est le reste d'une écriture interrompue : jamais relu,
            // il part quel que soit son âge.
            let abandoned = path.extension().and_then(|ext| ext.to_str()) == Some("tmp");
            if abandoned || age > INFO_DISK_MAX_AGE {
                if std::fs::remove_file(&path).is_ok() {
                    removed += 1;
                }
                continue;
            }
            kept.push((written, path));
        }
        if kept.len() > INFO_DISK_MAX_FILES {
            kept.sort_by_key(|(written, _)| *written);
            let extra = kept.len() - INFO_DISK_MAX_FILES;
            for (_, path) in kept.into_iter().take(extra) {
                if std::fs::remove_file(&path).is_ok() {
                    removed += 1;
                }
            }
        }
        removed
    }

    /// Prépare en tâche de fond les fiches des jeux les plus récemment joués :
    /// leur première ouverture ne paie plus la lecture réseau, elle relit le
    /// disque. Séquentiel et espacé, parce qu'un second logon Steam ferme la
    /// session courante (sociale comprise) ; le préchargement commence après
    /// le démarrage de l'interface, quand ses propres requêtes sont passées.
    pub fn start_info_preload(self: &Arc<Self>) {
        let shared = Arc::clone(self);
        std::thread::spawn(move || {
            shared.prune_game_info_cache();
            // Sans session enregistrée, il n'y a rien à lire : le jeu de
            // réglages de connexion n'a pas à payer cinq tentatives.
            if session::load_from(&shared.session_path()).is_none() {
                return;
            }
            std::thread::sleep(INFO_PRELOAD_DELAY);
            let recent = shared.recent_steam_games(INFO_PRELOAD_GAMES);
            for (index, appid) in recent.into_iter().enumerate() {
                if index > 0 {
                    std::thread::sleep(INFO_PRELOAD_GAP);
                }
                if shared.info_is_warm(appid) {
                    continue;
                }
                // Une lecture déjà en cours (l'utilisateur vient d'ouvrir la
                // même fiche) n'est pas doublée.
                let started = shared
                    .game_reads
                    .lock()
                    .map(|mut cache| cache.begin("info", appid))
                    .unwrap_or(false);
                if !started {
                    continue;
                }
                let value = shared.read_game_info(appid);
                let saved = value
                    .as_ref()
                    .ok()
                    .is_some_and(|value| Self::save_sheet(&shared.game_info_path(appid), value));
                if let Ok(mut cache) = shared.game_reads.lock() {
                    match &value {
                        Ok(value) => cache.finish("info", appid, value.clone(), saved),
                        Err(_) => cache.abandon("info", appid),
                    }
                }
            }
        });
    }

    /// Jeux Steam joués le plus récemment, du plus récent au plus ancien :
    /// c'est l'ordre dans lequel une fiche est rouverte.
    fn recent_steam_games(&self, limit: usize) -> Vec<u32> {
        let mut played: Vec<(i64, u32)> = self
            .store
            .load_games()
            .unwrap_or_default()
            .into_iter()
            .filter(|game| game.store == lapinux_core::stores::Store::Steam)
            .filter_map(|game| game.last_played.map(|at| (at, game.appid)))
            .collect();
        played.sort_by(|a, b| b.cmp(a));
        let mut seen = HashSet::new();
        played
            .into_iter()
            .filter(|(at, appid)| *at > 0 && seen.insert(*appid))
            .map(|(_, appid)| appid)
            .take(limit)
            .collect()
    }

    /// La fiche du jeu est déjà sur disque et assez jeune pour ne pas être
    /// relue au démarrage.
    fn info_is_warm(&self, appid: u32) -> bool {
        let Ok(meta) = std::fs::metadata(self.game_info_path(appid)) else {
            return false;
        };
        meta.modified()
            .ok()
            .and_then(|written| written.elapsed().ok())
            .map_or(false, |age| age < INFO_PRELOAD_MAX_AGE)
    }

    fn read_game_info(&self, appid: u32) -> Result<serde_json::Value> {
        let sources = self.steam_sources();
        // Le build installé se lit dans le manifeste : c'est lui que la fiche
        // compare au build public pour dire si une mise à jour attend.
        let installed = self
            .store
            .load_games()
            .unwrap_or_default()
            .into_iter()
            .find(|game| game.appid == appid);
        let installed_build = installed
            .as_ref()
            .and_then(|game| game.build_id.trim().parse::<u64>().ok());
        let mut info = lapinux_steam::gamedata::game_info(appid, &sources, installed_build);
        // Le manifeste porte aussi la date de ce build : elle date la version
        // qui tourne, comme `timebuildupdated` date celle que Steam publie.
        info.update.installed_build_time = installed
            .as_ref()
            .map(|game| game.last_updated)
            .filter(|time| *time > 0);
        // Contenus possédés : le catalogue tient la liste des licences, la
        // fiche ne la lit pas. Sans cette marque, la liste des contenus ne
        // pouvait pas dire « 4 sur 9 ».
        if !info.update.dlc.is_empty() {
            let catalog = lapinux_steam::catalog::Catalog::load(&self.catalog_path());
            let owned: HashSet<u32> = catalog
                .entries
                .iter()
                .find(|entry| entry.appid == appid)
                .map(|entry| {
                    catalog
                        .dlcs_of(entry)
                        .into_iter()
                        .map(|dlc| dlc.appid)
                        .collect()
                })
                .unwrap_or_default();
            for content in &mut info.update.dlc {
                content.owned = owned.contains(&content.appid);
            }
        }
        serde_json::to_value(&info)
            .map_err(|error| lapinux_core::Error::Other(format!("fiche illisible : {error}")))
    }

    /// Succès d'un jeu, lus par la session Steam seule : le schéma du jeu par
    /// `Player.GetGameAchievements`, l'état du compte par `ClientGetUserStats`.
    /// La session enregistrée suffit. Le résultat est gardé cinq minutes :
    /// la fiche les redemande à chaque repeint.
    pub fn game_achievements(self: &Arc<Self>, appid: u32) -> Result<Envelope> {
        let shared = Arc::clone(self);
        self.cached_remote_read("succès", appid, move || {
            let sources = shared.steam_sources();
            let read = lapinux_steam::achievements::game_achievements(appid, &sources);
            serde_json::to_value(&read)
                .map_err(|error| lapinux_core::Error::Other(format!("succès illisibles : {error}")))
        })
    }

    /// Relevé frais des succès, sans le cache de cinq minutes : la surveillance
    /// d'une partie doit voir un déblocage sans attendre que la fiche le
    /// redemande pour un autre jeu.
    pub(crate) fn read_steam_achievements_now(
        &self,
        appid: u32,
    ) -> lapinux_core::model::SteamAchievements {
        let sources = self.steam_sources();
        lapinux_steam::achievements::game_achievements(appid, &sources)
    }

    /// Session enregistrée et clé du compte, ce dont les lectures Steam ont
    /// besoin.
    fn steam_sources(&self) -> lapinux_steam::gamedata::Sources {
        let saved = session::load_from(&self.session_path());
        // La langue choisie pour les jeux pilote aussi la langue des pages
        // Steam lues pour la fiche (résumé, à propos, annonces).
        let language = lapinux_core::Config::load(&self.paths.config_file)
            .map(|config| config.general.game_language)
            .unwrap_or_default();
        lapinux_steam::gamedata::Sources {
            language,
            account: saved
                .as_ref()
                .map(|saved| saved.account_name.clone())
                .unwrap_or_default(),
            refresh_token: saved
                .as_ref()
                .map(|saved| saved.refresh_token.clone())
                .unwrap_or_default(),
            steam_id: saved.as_ref().map(|saved| saved.steam_id).unwrap_or(0),
        }
    }

    /// Débloque des succès par la session partagée du daemon.
    ///
    /// Le pont Steamworks d'un jeu n'ouvre pas de connexion à lui : Steam ne
    /// garde qu'une session par compte, et la seconde fermerait celle qui sert
    /// aux lectures, aux bandeaux et à la synchronisation.
    pub(crate) fn store_steam_achievements(
        &self,
        appid: u32,
        names: &[String],
    ) -> Result<serde_json::Value> {
        self.store_steam_achievements_for(appid, names, None)
    }

    fn store_steam_achievements_for(
        &self,
        appid: u32,
        names: &[String],
        owner: Option<u64>,
    ) -> Result<serde_json::Value> {
        if names.is_empty() {
            return Ok(serde_json::json!({ "written": [], "already": [], "unknown": [] }));
        }
        // Lire une seule fois : l'identité vérifiée doit être celle qui sert
        // réellement à l'envoi, même si la session change entre-temps.
        let sources = self.steam_sources();
        if owner.is_some_and(|id| id == 0 || sources.steam_id != id) {
            return Err(lapinux_core::Error::Other(
                "compte Steam différent : lot conservé".into(),
            ));
        }
        if !sources.has_session() {
            return Err(lapinux_core::Error::Other(
                "compte Steam non connecté : succès non enregistrés".into(),
            ));
        }
        let report = lapinux_steam::stats_store::store_achievements_shared(
            &sources.account,
            &sources.refresh_token,
            sources.steam_id,
            appid,
            names,
        )
        .map_err(|error| lapinux_core::Error::Other(error.to_string()))?;
        // Un lot entièrement déjà débloqué n'écrit rien : ce n'est pas un
        // refus, et le pont ne doit pas rouvrir une session pour rien.
        if !report.written.is_empty()
            && (!report.accepted || !report.failed.is_empty() || report.out_of_date)
        {
            return Err(lapinux_core::Error::Other(format!(
                "Steam a refusé l'enregistrement des succès (eresult {}, schéma périmé : {}, statistiques refusées : {})",
                report.eresult, report.out_of_date, report.failed.len()
            )));
        }
        if !report.unknown.is_empty() {
            return Err(lapinux_core::Error::Other(format!(
                "succès inconnus du schéma Steam : {}",
                report.unknown.join(", ")
            )));
        }
        tracing::info!(
            "succès de {appid} enregistrés par la session partagée : {} nouveau(x), {} déjà là",
            report.written.len(),
            report.already.len()
        );
        Ok(serde_json::json!({
            "written": report.written,
            "already": report.already,
            "unknown": report.unknown,
            "eresult": report.eresult,
        }))
    }

    /// Vide la file des ponts : un lot en échec reste pour le tour suivant.
    pub(crate) fn drain_achievement_spool(self: &Arc<Self>) {
        let dir = self.paths.data_dir.join("achievements");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            // Demande de lobby du pont Windows : la réponse repart dans un
            // fichier voisin, que le DLL attend.
            if serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|value| value.get("lobby_request").and_then(|flag| flag.as_bool()))
                == Some(true)
            {
                #[derive(serde::Deserialize)]
                struct Ask {
                    appid: u32,
                    max_members: u32,
                }
                let ask = serde_json::from_str::<Ask>(&text).ok();
                if let Some(ask) = ask.as_ref() {
                    tracing::info!(
                        "pont Windows : lobby demandé pour appid {} ({} places)",
                        ask.appid,
                        ask.max_members
                    );
                }
                let appid = ask.as_ref().map_or(0, |ask| ask.appid);
                let verdict = ask.and_then(|ask| {
                    let social = self.ensure_social().ok()?;
                    lapinux_steam::social::create_lobby(
                        &social,
                        ask.appid,
                        ask.max_members,
                        None,
                        None,
                        None,
                        crate::social::listen_address().as_deref(),
                    )?
                    .recv_timeout(std::time::Duration::from_secs(12))
                    .ok()
                });
                if let (Some(Ok(lobby)), Some(address)) =
                    (&verdict, crate::social::listen_address())
                {
                    if let Ok(social) = self.ensure_social() {
                        lapinux_steam::social::set_lobby_data(&social, appid, *lobby, &address);
                    }
                }
                match &verdict {
                    Some(Ok(lobby)) => {
                        tracing::info!("pont Windows : lobby {lobby} créé pour appid {appid}")
                    }
                    Some(Err(eresult)) => tracing::warn!(
                        "pont Windows : lobby refusé pour appid {appid} (eresult {eresult})"
                    ),
                    None => tracing::warn!(
                        "pont Windows : aucun lobby pour appid {appid} (session ou délai)"
                    ),
                }
                let body = match verdict {
                    Some(Ok(lobby)) => format!("{{\"ok\":true,\"lobby\":{lobby}}}"),
                    Some(Err(eresult)) => format!("{{\"ok\":false,\"eresult\":{eresult}}}"),
                    None => "{\"ok\":false,\"eresult\":2}".to_string(),
                };
                let reply = path.with_file_name(format!(
                    "{}.reply",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ));
                let _ = std::fs::write(&reply, body);
                let _ = std::fs::remove_file(&path);
                continue;
            }
            #[derive(serde::Deserialize)]
            struct Lot {
                appid: u32,
                names: Vec<String>,
                #[serde(default)]
                steam_id: Option<u64>,
            }
            let Ok(lot) = serde_json::from_str::<Lot>(&text) else {
                tracing::warn!("lot de succes illisible : {}", path.display());
                let _ = std::fs::remove_file(&path);
                continue;
            };
            if path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("native-"))
                && lot.steam_id.is_none()
            {
                tracing::warn!("lot natif sans SteamID conservé : {}", path.display());
                continue;
            }
            // Les anciens lots Windows n'ont pas de SteamID ; les lots natifs
            // sont conservés si le compte a changé depuis la partie.
            match self.store_steam_achievements_for(lot.appid, &lot.names, lot.steam_id) {
                Ok(_) => {
                    let _ = std::fs::remove_file(&path);
                }
                Err(error) => tracing::warn!(
                    "lot de succes de {} garde pour plus tard : {error}",
                    lot.appid
                ),
            }
        }
    }

    pub(super) fn catalog_path(&self) -> std::path::PathBuf {
        self.paths.data_dir.join("steam-catalog.json")
    }

    /// Synchronisation du catalogue en arrière-plan, une seule à la fois.
    pub(crate) fn start_catalog_refresh(self: &Arc<Self>, account: &str) {
        if account.trim().is_empty() || self.catalog_running.swap(true, Ordering::SeqCst) {
            return;
        }
        let shared = Arc::clone(self);
        let account = account.trim().to_string();
        std::thread::spawn(move || {
            let result = shared.refresh_catalog(&account);
            if let Ok(mut slot) = shared.catalog_error.lock() {
                *slot = result.err().map(|error| error.to_string());
            }
            shared.catalog_running.store(false, Ordering::SeqCst);
        });
    }

    /// Vérification légère du compte au démarrage : `Player.GetOwnedGames`
    /// rend les appids en un appel. La liste est comparée au relevé du
    /// démarrage précédent et au catalogue : seuls les appids apparus depuis
    /// sont nommés, un par un, sur la boutique publique — pas de relecture des
    /// paquets, des dates ni des fiches. Un jeu ajouté au compte entre deux
    /// lancements est ainsi ajouté seul, sans relancer tout le catalogue, et
    /// les outils ou playtests déjà présents au premier relevé ne rentrent pas
    /// dans la bibliothèque. La synchronisation complète reste le filet quand
    /// le cache est vide ou trop ancien.
    pub(crate) fn start_account_probe(self: &Arc<Self>) {
        let account = self.config_snapshot().steam.username.trim().to_string();
        if account.is_empty() {
            tracing::info!("catalogue : aucun compte configuré, vérification sautée");
            return;
        }
        // Un cache vide relève de la synchronisation complète : le contrôle
        // léger n'a pas de base de comparaison et coûterait une session de
        // plus au premier démarrage.
        if lapinux_steam::catalog::Catalog::load(&self.catalog_path())
            .entries
            .is_empty()
        {
            tracing::info!(
                "catalogue : cache vide, contrôle léger laissé à la synchronisation complète"
            );
            return;
        }
        let key = format!("{ACCOUNT_PROBE_KEY}.{}", account.to_ascii_lowercase());
        let now = lapinux_core::util::now_unix();
        if let Ok(Some(last)) = self.store.get_json::<i64>(&key) {
            let elapsed = now.saturating_sub(last);
            if elapsed < ACCOUNT_PROBE_INTERVAL {
                tracing::info!("catalogue : contrôle du compte fait il y a {elapsed} s, sauté");
                return;
            }
        }
        if self.account_probe_running.swap(true, Ordering::SeqCst) {
            return;
        }
        // La date est posée avant l'appel : un refus de débit ne doit pas être
        // retenté à chaque relance rapprochée.
        if let Err(error) = self.store.set_json(&key, &now) {
            tracing::debug!("catalogue : date du contrôle non enregistrée ({error})");
        }
        let shared = Arc::clone(self);
        std::thread::spawn(move || {
            std::thread::sleep(ACCOUNT_PROBE_DELAY);
            tracing::info!("catalogue : vérification légère du compte");
            if let Err(error) = shared.account_probe(&account) {
                tracing::info!("catalogue : vérification du compte ignorée ({error})");
            }
            shared.account_probe_running.store(false, Ordering::SeqCst);
        });
    }

    fn account_probe(&self, account: &str) -> Result<()> {
        let Some(saved) = self.saved_session(account) else {
            tracing::info!("catalogue : aucune session Steam enregistrée pour {account}");
            return Ok(());
        };
        if saved.steam_id == 0 || saved.refresh_token.trim().is_empty() {
            tracing::info!("catalogue : session Steam incomplète, vérification sautée");
            return Ok(());
        }
        // Une seule lecture, un seul appel : la session partagée du compte est
        // réutilisée. Pas de pause sociale, donc rien à arrêter ni à relancer.
        self.account_probe_work(account, &saved)
    }

    fn account_probe_work(
        &self,
        account: &str,
        saved: &lapinux_steam::session::SavedSession,
    ) -> Result<()> {
        let mut appids = match lapinux_steam::playtime::owned_appids(
            account,
            &saved.refresh_token,
            saved.steam_id,
        ) {
            Ok(appids) => appids,
            Err(first) => {
                // La session partagée peut avoir été coupée par un logon
                // voisin : une seconde lecture repart sur une session neuve.
                tracing::info!(
                    "catalogue : première lecture du compte échouée ({first}), nouvel essai"
                );
                std::thread::sleep(std::time::Duration::from_secs(2));
                lapinux_steam::playtime::owned_appids(account, &saved.refresh_token, saved.steam_id)
                    .map_err(|error| {
                        lapinux_core::Error::Other(format!("bibliothèque du compte : {error}"))
                    })?
            }
        };
        appids.sort_unstable();
        appids.dedup();
        tracing::info!("catalogue : {} appid(s) reçus du compte", appids.len());
        let seen_key = format!("{ACCOUNT_APPIDS_KEY}.{}", account.to_ascii_lowercase());
        let previous: HashSet<u32> = self
            .store
            .get_json::<Vec<u32>>(&seen_key)
            .ok()
            .flatten()
            .unwrap_or_default()
            .into_iter()
            .collect();
        let path = self.catalog_path();
        let mut catalog = lapinux_steam::catalog::Catalog::load(&path);
        // Un cache vide relève de la synchronisation complète : le contrôle
        // léger n'a pas de base de comparaison fiable.
        if catalog.entries.is_empty() {
            self.save_account_appids(&seen_key, &appids);
            return Ok(());
        }
        // Premier relevé : il devient la base, sans rien ajouter. Sans cela,
        // les outils, playtests et bêtas que `GetOwnedGames` rend aussi — et
        // que la synchro complète écarte — entreraient dans la bibliothèque.
        if previous.is_empty() {
            self.save_account_appids(&seen_key, &appids);
            tracing::info!(
                "catalogue : base de {} appid(s) du compte posée, aucun ajout",
                appids.len()
            );
            return Ok(());
        }
        let known: HashSet<u32> = catalog
            .entries
            .iter()
            .map(|entry| entry.appid)
            .chain(catalog.dlcs.iter().map(|dlc| dlc.appid))
            .collect();
        let missing = new_account_appids(&previous, &known, &appids);
        if missing.is_empty() {
            self.save_account_appids(&seen_key, &appids);
            tracing::info!("catalogue : compte à jour, aucun nouveau jeu");
            return Ok(());
        }
        let now = lapinux_core::util::now_unix();
        let mut added = 0usize;
        // Un appid dont la boutique n'a rien voulu dire reste hors du relevé :
        // le démarrage suivant le redemandera. Un outil, lui, entre dans le
        // relevé sans entrer dans le catalogue.
        let mut retry: HashSet<u32> = HashSet::new();
        for appid in missing {
            let Some((name, kind)) = lapinux_steam::catalog::Catalog::store_entry(appid) else {
                retry.insert(appid);
                continue;
            };
            if !(kind.eq_ignore_ascii_case("game") || kind.eq_ignore_ascii_case("demo")) {
                continue;
            }
            catalog.entries.push(lapinux_steam::catalog::Entry {
                appid,
                name: name.clone(),
                kind,
                ..Default::default()
            });
            catalog.stamped.insert(appid, now);
            added += 1;
            tracing::info!(
                "catalogue : jeu {appid} « {name} » ajouté (vérification légère du compte)"
            );
        }
        if added > 0 {
            // Le cache reste « frais » : le prochain démarrage ne relancera
            // pas la grosse synchronisation à cause de cette seule addition.
            catalog.updated = now;
            catalog.save(&path).map_err(|error| {
                lapinux_core::Error::Other(format!("catalogue non enregistré : {error}"))
            })?;
            tracing::info!("catalogue : {added} jeu(x) ajouté(s) sans relire le compte");
        }
        let baseline: Vec<u32> = appids
            .iter()
            .copied()
            .filter(|appid| !retry.contains(appid))
            .collect();
        self.save_account_appids(&seen_key, &baseline);
        Ok(())
    }

    fn save_account_appids(&self, key: &str, appids: &[u32]) {
        if let Err(error) = self.store.set_json(key, &appids) {
            tracing::info!("catalogue : relevé du compte non enregistré ({error})");
        }
    }

    pub fn steam_catalog_patch(self: &Arc<Self>, appid: u32, name: String) -> Result<Envelope> {
        let mut appid = appid;
        let mut title = name.trim().to_string();
        if appid == 0 && !title.is_empty() {
            if let Some((found, named)) = lapinux_steam::native::search_appid(&title) {
                appid = found;
                title = named;
            }
        }
        if appid != 0 {
            self.insert_catalog_entry(appid, title)?;
            // La fiche d'un jeu obtenu par une clé n'a pas encore d'images :
            // la boutique les donne tout de suite, sans session.
            let shared = Arc::clone(self);
            std::thread::spawn(move || {
                shared.fill_catalog_details(appid);
                // Description, captures, prérequis, poids : la fiche complète
                // se lit tout de suite, sinon elle restait vide jusqu'à la
                // synchronisation suivante.
                if let Err(error) = shared.game_info(appid) {
                    tracing::info!("fiche du jeu {appid} non lue ({error})");
                }
            });
        } else {
            // Le reçu ne nomme rien : la licence de la clé porte son paquet, et
            // la boutique donne le jeu en une requête. La lecture prend
            // quelques secondes, hors de la réponse.
            let shared = Arc::clone(self);
            let account = self.config_snapshot().steam.username.trim().to_string();
            std::thread::spawn(move || match shared.resolve_key_game() {
                Some((found, named)) => {
                    if let Err(error) = shared.insert_catalog_entry(found, named) {
                        tracing::warn!("catalogue : jeu de la clé non enregistré ({error})");
                    } else {
                        shared.fill_catalog_details(found);
                        if let Err(error) = shared.game_info(found) {
                            tracing::info!("fiche du jeu {found} non lue ({error})");
                        }
                    }
                }
                None => {
                    // Ni reçu ni boutique n'ont nommé le jeu : la relecture
                    // complète reste le filet, en tâche de fond.
                    tracing::warn!("catalogue : jeu de la clé introuvable, relecture complète");
                    shared.start_catalog_refresh(&account);
                }
            });
        }
        self.steam_catalog(false)
    }

    /// Ajoute une fiche minimale au catalogue, sans relire le compte : la
    /// synchronisation complète la complétera.
    /// Complète la fiche d'un jeu ajouté par une clé : images, icône et
    /// présentation lues sur la boutique publique, sans session. Sans cela, sa
    /// fiche restait vide jusqu'à la synchronisation suivante.
    fn fill_catalog_details(&self, appid: u32) {
        let Some(details) = lapinux_steam::native::store_art(appid) else {
            tracing::info!("catalogue : images du jeu {appid} indisponibles");
            return;
        };
        let path = self.catalog_path();
        let mut catalog = lapinux_steam::catalog::Catalog::load(&path);
        let Some(slot) = catalog
            .entries
            .iter_mut()
            .find(|entry| entry.appid == appid)
        else {
            return;
        };
        if !slot.details.header_image.is_empty() {
            return;
        }
        slot.details = details;
        catalog.updated = lapinux_core::util::now_unix();
        if let Err(error) = catalog.save(&path) {
            tracing::warn!("catalogue : fiche du jeu {appid} non enregistrée ({error})");
        } else {
            tracing::info!("catalogue : images du jeu {appid} complétées");
        }
    }

    fn insert_catalog_entry(&self, appid: u32, title: String) -> Result<()> {
        let path = self.catalog_path();
        let mut catalog = lapinux_steam::catalog::Catalog::load(&path);
        let title = title.trim().to_string();
        let stamp = lapinux_core::util::now_unix();
        if catalog.entries.iter().any(|entry| entry.appid == appid) {
            // La fiche existait déjà, sans date : la clé vient de la lui
            // donner, et cette date prime sur celles qu'une relève devine.
            let mut touched = false;
            if let Some(slot) = catalog
                .entries
                .iter_mut()
                .find(|entry| entry.appid == appid)
            {
                if slot.acquired.is_none() {
                    slot.acquired = Some(stamp);
                    touched = true;
                }
                if !title.is_empty() && slot.name.starts_with("Jeu ") {
                    slot.name = title;
                    touched = true;
                }
            }
            catalog.stamped.insert(appid, stamp);
            catalog.updated = stamp;
            catalog
                .save(&path)
                .map_err(|error| lapinux_core::Error::Other(error.to_string()))?;
            if touched {
                tracing::info!("catalogue : jeu {appid} daté après une clé");
            }
            return Ok(());
        }
        let title = if title.is_empty() {
            format!("Jeu {appid}")
        } else {
            title
        };
        catalog.entries.push(lapinux_steam::catalog::Entry {
            appid,
            name: title,
            kind: "Game".to_string(),
            family: false,
            acquired: Some(stamp),
            minutes: None,
            details: Default::default(),
        });
        catalog.stamped.insert(appid, stamp);
        catalog.updated = stamp;
        catalog
            .save(&path)
            .map_err(|error| lapinux_core::Error::Other(error.to_string()))?;
        tracing::info!("catalogue : jeu {appid} ajouté après une clé");
        Ok(())
    }

    /// Jeu de la licence la plus récente du compte : sa date départage la clé
    /// qui vient d'être activée, et son paquet suffit à la boutique pour rendre
    /// l'appid et le nom. Ni PICS ni reconstruction du catalogue.
    fn resolve_key_game(self: &Arc<Self>) -> Option<(u32, String)> {
        let account = self.config_snapshot().steam.username.trim().to_string();
        if account.is_empty() {
            tracing::warn!("catalogue : aucun compte Steam en configuration");
            return None;
        }
        let saved = self.saved_session(&account)?;
        if saved.refresh_token.trim().is_empty() {
            tracing::warn!("catalogue : session Lapinux sans jeton réutilisable");
            return None;
        }
        tracing::info!("catalogue : recherche du jeu de la clé pour {account}");
        self.pause_social();
        let result = (|| {
            let mut session = self
                .sign_in_with_token(&account, &saved.refresh_token)
                .ok()?;
            tracing::info!("catalogue : session ouverte pour la clé");
            let mut licenses = session.licenses.take().unwrap_or_default();
            drop(session);
            licenses.retain(|license| license.time_created > 0);
            let newest = licenses.iter().max_by_key(|license| license.time_created)?;
            tracing::info!(
                "catalogue : paquet {} de la licence la plus récente",
                newest.package_id
            );
            lapinux_steam::native::package_app(newest.package_id)
        })();
        self.resume_social();
        if result.is_none() {
            tracing::warn!("catalogue : la licence de la clé n'a rendu aucun jeu");
        }
        result
    }

    /// Travail lourd de la synchronisation, exécuté hors de la boucle IPC.
    /// La liste de licences vient du canal CM Lapinux ; PICS étend chaque
    /// paquet en appids, puis rend les noms. SteamCMD n'est jamais appelé.
    ///
    /// Steam ne garde qu'une session par compte : deux logons qui se suivent
    /// se ferment l'un l'autre, et la session sociale se reconnecte toute
    /// seule — ce qui coupe la synchronisation en plein vol. Elle attend donc
    /// la fin du catalogue, puis repart.
    pub(super) fn refresh_catalog(self: &Arc<Self>, account: &str) -> Result<()> {
        self.pause_social();
        let result = self.refresh_catalog_work(account);
        self.resume_social();
        result
    }

    fn refresh_catalog_work(&self, account: &str) -> Result<()> {
        let saved = self.saved_session(account).ok_or_else(|| {
            lapinux_core::Error::Other(
                "connectez votre compte Steam dans Réglages → Compte Steam".into(),
            )
        })?;
        if saved.refresh_token.trim().is_empty() {
            return Err(lapinux_core::Error::Other(
                "la session Lapinux n'a pas de jeton réutilisable : reconnectez-vous".into(),
            ));
        }
        let mut session = self
            .sign_in_with_token(account, &saved.refresh_token)
            .map_err(|error| lapinux_core::Error::Other(format!("session Lapinux : {error}")))?;
        let steam_id = session.steam_id;
        let licenses = session
            .licenses
            .take()
            .filter(|licenses| !licenses.is_empty())
            .ok_or_else(|| {
                lapinux_core::Error::Other("Steam n'a renvoyé aucune licence pour ce compte".into())
            })?;
        let own_account_id = (steam_id & 0xffff_ffff) as u32;
        let (packages, family_packages) = split_license_packages(&licenses, own_account_id);
        // Le canal qui a lu les licences se ferme avant que le catalogue
        // n'ouvre sa propre session Steam : un compte n'en garde qu'une, et
        // deux logons qui se suivent se coupent l'un l'autre — c'est ainsi que
        // PICS cessait de rendre les fiches et vidait la bibliothèque.
        drop(session);
        // Les licences du site portent la date d'achat que le protocole ne donne
        // pas. Elles se lisent ici : une session close invalide le jeton qu'elle
        // a émis, et la page rend alors 403. Rien n'est bloquant — sans elle,
        // la synchro continue, les jeux restent simplement sans date.
        // Steam renouvelle le jeton d'actualisation à chaque connexion : celui
        // chargé en tête de synchro est périmé depuis le logon, et un jeton
        // périmé fait rendre 403 à la page. On relit donc celui du disque.
        let current_session = lapinux_steam::session::load_from(&lapinux_steam::session::path_in(
            &self.paths.data_dir,
        ));
        let refresh_token = current_session
            .as_ref()
            .map(|session| session.refresh_token.as_str())
            .filter(|token| !token.trim().is_empty())
            .unwrap_or(&saved.refresh_token);
        let site_dates = match lapinux_steam::web::license_dates(account, refresh_token, steam_id) {
            Ok(dates) => dates,
            Err(error) => {
                tracing::info!("catalogue : dates du site indisponibles ({error})");
                Vec::new()
            }
        };
        // Une licence datée date le jeu principal de son paquet : c'est la date
        // d'ajout au compte. Les paquets de la famille datent le compte de
        // l'autre membre : le service de famille fournit à part leur date
        // (`rt_time_acquired`).
        let mut package_dates: std::collections::HashMap<u32, i64> =
            std::collections::HashMap::new();
        for license in &licenses {
            if license.time_created <= 0 {
                continue;
            }
            package_dates
                .entry(license.package_id)
                .and_modify(|old| *old = (*old).min(license.time_created))
                .or_insert(license.time_created);
        }
        tracing::info!(
            "catalogue : {} paquets de licences, {} partagés par la famille Steam",
            packages.len(),
            family_packages.len()
        );
        let path = self.catalog_path();
        let previous = lapinux_steam::catalog::Catalog::load(&path);
        let (mut entries, mut dlcs, _package_main, appids, family_dlcs) = native::catalog(
            account,
            &saved.refresh_token,
            steam_id,
            &packages,
            &family_packages,
            &package_dates,
            &site_dates,
        )
        .map_err(|e| lapinux_core::Error::Other(e.to_string()))?;
        if entries.is_empty() {
            return Err(lapinux_core::Error::Other(
                "Steam n'a renvoyé aucun jeu pour ce compte".into(),
            ));
        }
        // Une lecture du site qui échoue (403 d'Edge, page vide) ne doit pas
        // effacer les dates qu'une synchronisation précédente avait posées :
        // sans cette reprise, une panne d'une minute faisait reculer la
        // bibliothèque datée.
        // Le report tourne toujours : une lecture partielle du site ne doit
        // pas faire reculer un jeu daté la veille.
        if !previous.entries.is_empty() {
            let earlier: std::collections::HashMap<u32, (i64, bool)> = previous
                .entries
                .iter()
                .filter_map(|entry| Some((entry.appid, (entry.acquired?, entry.family))))
                .collect();
            let mut kept = 0usize;
            for entry in &mut entries {
                if entry.acquired.is_none() {
                    if let Some((stamp, was_family)) = earlier.get(&entry.appid) {
                        entry.acquired = Some(*stamp);
                        // Une date reprise d'une licence du compte prouve la
                        // possession. Une date de famille, elle, ne doit pas
                        // transformer un jeu prêté en jeu possédé.
                        entry.family = entry.family && *was_family;
                        kept += 1;
                    }
                }
            }
            if kept > 0 {
                tracing::info!(
                    "catalogue : {kept} date(s) gardées de la synchronisation précédente"
                );
            }
        }
        // Les dates du site comblent les jeux arrivés par la liste des jeux
        // possédés, comme Insurgency. La page n'a pas d'appid, d'où le
        // rapprochement par le nom — et seulement pour les jeux sans date.
        if !site_dates.is_empty() {
            let mut filled = 0usize;
            for entry in &mut entries {
                if entry.acquired.is_some() || entry.family {
                    continue;
                }
                if let Some((_, stamp)) = site_dates
                    .iter()
                    .find(|(name, _)| lapinux_steam::web::same_title(name, &entry.name))
                {
                    entry.acquired = Some(*stamp);
                    // La page des licences ne liste que les licences *du
                    // compte* : un homonyme daté prouve la possession. Un jeu
                    // acheté (clé revendeur comprise) garde donc sa date même
                    // si un membre de la famille le partage aussi — la
                    // possession l'emporte sur le partage.
                    entry.family = false;
                    filled += 1;
                }
            }
            if filled > 0 {
                tracing::info!(
                    "catalogue : {filled} date(s) d'achat lues sur la page des licences"
                );
            } else {
                tracing::info!(
                    "catalogue : {} licence(s) datées sur le site, aucune ne rapproche un jeu sans date",
                    site_dates.len()
                );
            }
        }
        // L'historique des transactions nomme les jeux achetés par leur appid,
        // là où la page des licences n'a que des noms de paquets : il date les
        // jeux qu'aucun rapprochement de noms n'atteint. Une lecture refusée ne
        // retire aucune date.
        if entries.iter().any(|entry| entry.acquired.is_none()) {
            match lapinux_steam::web::purchase_dates(account, refresh_token, steam_id) {
                Ok(dates) if !dates.is_empty() => {
                    let known: std::collections::HashMap<u32, i64> = dates.into_iter().collect();
                    let mut filled = 0usize;
                    for entry in &mut entries {
                        if entry.acquired.is_some() {
                            continue;
                        }
                        if let Some(stamp) = known.get(&entry.appid) {
                            entry.acquired = Some(*stamp);
                            entry.family = false;
                            filled += 1;
                        }
                    }
                    tracing::info!("catalogue : {filled} jeu(x) datés par l'historique des achats");
                }
                Ok(_) => {
                    tracing::info!("catalogue : historique des achats sans appid exploitable")
                }
                Err(error) => {
                    tracing::info!("catalogue : historique des achats indisponible ({error})")
                }
            }
        }
        // Une synchronisation incomplète ne doit pas effacer une bibliothèque
        // connue : les fiches des jeux toujours licenciés que PICS n'a pas
        // redécrites cette fois restent telles quelles.
        if !previous.entries.is_empty() {
            let licensed: std::collections::HashSet<u32> = appids.iter().copied().collect();
            let known: std::collections::HashSet<u32> = entries
                .iter()
                .map(|entry| entry.appid)
                .chain(dlcs.iter().map(|dlc| dlc.appid))
                .collect();
            let mut kept = 0usize;
            for entry in &previous.entries {
                if licensed.contains(&entry.appid) && !known.contains(&entry.appid) {
                    entries.push(entry.clone());
                    kept += 1;
                }
            }
            for dlc in &previous.dlcs {
                if licensed.contains(&dlc.appid) && !known.contains(&dlc.appid) {
                    dlcs.push(dlc.clone());
                }
            }
            if kept > 0 {
                tracing::info!(
                    "catalogue : {kept} fiche(s) gardées de la synchronisation précédente"
                );
            }
        }
        // Une synchronisation qui ne décrit qu'une poignée des applications
        // licenciées est dégradée, même si l'ancien catalogue est déjà court :
        // le 1er novembre, une session CM muette a fait tomber la bibliothèque
        // de 1 362 à 159 jeux, et chaque relance réécrivait le catalogue déjà
        // amputé au lieu de garder le précédent. Le rapport se mesure donc aux
        // applications licenciées, pas à la taille du catalogue d'avant.
        let named = entries.len() + dlcs.len();
        if !appids.is_empty() && named * 2 < appids.len() {
            return Err(lapinux_core::Error::Other(format!(
                "Steam n'a décrit que {named} applications sur {} : l'ancien catalogue est gardé",
                appids.len()
            )));
        }
        entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        entries.dedup_by_key(|entry| entry.appid);
        dlcs.sort_by_key(|entry| entry.appid);
        dlcs.dedup_by_key(|entry| entry.appid);
        // Une clé peut désigner un jeu que la relève ne voit pas (paquet sans
        // fiche PICS, jeu non encore publié) : sa fiche déjà connue est reprise,
        // sinon la synchronisation complète l'effacerait de la bibliothèque.
        keep_stamped_entries(&mut entries, &previous);
        // Les dates posées à l'activation traversent la relève : elles sont
        // plus fiables que celles que les licences et les lots font deviner.
        let stamped = previous.stamped.clone();
        for entry in &mut entries {
            if let Some(date) = stamped.get(&entry.appid) {
                entry.acquired = Some(*date);
            }
        }
        let catalog = lapinux_steam::catalog::Catalog {
            version: lapinux_steam::catalog::VERSION,
            account: account.to_string(),
            updated: lapinux_core::util::now_unix(),
            entries,
            dlcs,
            family_dlcs,
            stamped,
        };
        catalog
            .save(&path)
            .map_err(|e| lapinux_core::Error::Other(format!("catalogue non enregistré : {e}")))?;
        tracing::info!(
            "catalogue : {} jeux et {} contenus additionnels enregistrés",
            catalog.entries.len(),
            catalog.dlcs.len()
        );
        Ok(())
    }
}

#[cfg(test)]
mod account_probe_tests {
    use super::*;

    #[test]
    fn native_spool_keeps_batches_when_account_changes_or_session_is_unavailable() {
        let root = tempfile::tempdir().unwrap();
        let paths = lapinux_core::paths::LapinuxPaths::under(root.path());
        paths.ensure().unwrap();
        let shared = std::sync::Arc::new(Shared::new(&paths).unwrap());
        let spool = paths.data_dir.join("achievements");
        std::fs::create_dir_all(&spool).unwrap();
        let lot = spool.join("native-test.json");
        std::fs::write(
            &lot,
            r#"{"appid":424242,"names":["lobby_request"],"steam_id":123}"#,
        )
        .unwrap();
        let missing_owner = spool.join("native-missing.json");
        std::fs::write(&missing_owner, r#"{"appid":424242,"names":["ACH_TEST"]}"#).unwrap();
        // Le pont Windows utilise toujours le format historique sans steam_id.
        let windows = spool.join("windows-legacy.json");
        std::fs::write(&windows, r#"{"appid":424242,"names":["ACH_WINDOWS"]}"#).unwrap();
        let session = |steam_id| lapinux_steam::session::SavedSession {
            account_name: "test".into(),
            steam_id,
            refresh_token: String::new(),
            ..Default::default()
        };
        lapinux_steam::session::save_to(&shared.session_path(), &session(456)).unwrap();
        let error = shared
            .store_steam_achievements_for(424242, &["ACH_TEST".into()], Some(123))
            .unwrap_err();
        assert!(error.to_string().contains("compte Steam différent"));
        shared.drain_achievement_spool();
        assert!(lot.exists());
        assert!(missing_owner.exists());
        lapinux_steam::session::save_to(&shared.session_path(), &session(123)).unwrap();
        shared.drain_achievement_spool();
        assert!(
            lot.exists(),
            "an unavailable session must not discard the lot"
        );
        assert!(
            windows.exists(),
            "legacy Windows lots must survive an unavailable session"
        );
    }

    #[test]
    fn seuls_les_appids_apparus_depuis_le_releve_et_absents_du_catalogue_sont_retenus() {
        let previous: HashSet<u32> = [10, 20, 30].into_iter().collect();
        let known: HashSet<u32> = [10, 20, 40].into_iter().collect();
        assert_eq!(
            new_account_appids(&previous, &known, &[10, 20, 30, 40, 50]),
            vec![50]
        );
    }

    #[test]
    fn la_synchro_complete_garde_le_jeu_d_une_cle() {
        let previous = lapinux_steam::catalog::Catalog {
            entries: vec![lapinux_steam::catalog::Entry {
                appid: 3_780_530,
                name: "American Cooking Simulator: Multiplayer".into(),
                kind: "Game".into(),
                acquired: Some(1_700_000_000),
                ..Default::default()
            }],
            stamped: [(3_780_530, 1_700_000_000)].into_iter().collect(),
            ..Default::default()
        };
        let mut entries = Vec::new();
        keep_stamped_entries(&mut entries, &previous);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].appid, 3_780_530);
    }
}
