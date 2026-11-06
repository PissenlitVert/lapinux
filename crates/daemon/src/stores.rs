//! Boutiques hors Steam : comptes, bibliothèques et installations.
//!
//! Les trois boutiques (Epic, GOG, itch.io) n'ont ni la même connexion ni les
//! mêmes capacités ; [`lapinux_stores`] uniformise les appels, ce module tient
//! l'état du daemon : le cache des bibliothèques, l'enregistrement des jetons
//! et les tâches d'installation qui atterrissent dans la bibliothèque Lapinux
//! comme les autres téléchargements.
//!
//! Steam reste à part : sa session et son catalogue vivent dans
//! [`crate::steam`], et [`Store::capabilities`] dit précisément ce que chaque
//! boutique expose — aucune des trois n'a de succès, itch n'a pas de cloud.

use crate::{Result, Shared};
use lapinux_core::ipc::Envelope;
use lapinux_core::model::{
    CloudFile, CloudReport, Drm, Game, GameOrigin, Job, JobKind, JobState,
    SteamAchievement, SteamAchievements,
};
use lapinux_core::stores::{self as core_stores, Store, StoreAccount, StoreLibrary, StoresView};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Clé du cache des bibliothèques dans la base du daemon.
const STORES_KEY: &str = "stores_view";

impl Shared {
    /// Jeu d'une boutique et la version installée (`linux`, `windows`…) : GOG
    /// range ses sauvegardes sous l'identifiant de client du build, qui dépend
    /// de la plateforme.
    fn store_game(&self, appid: u32) -> Result<(Game, String)> {
        let game = self
            .store
            .game(appid)
            .map_err(|error| lapinux_core::Error::Other(error.to_string()))?
            .ok_or_else(|| lapinux_core::Error::Other("jeu introuvable".into()))?;
        if game.store == Store::Steam {
            return Err(lapinux_core::Error::Other(
                "ce jeu s'ouvre depuis Steam".into(),
            ));
        }
        let platform = core_stores::StoreMark::read(&game.install_dir())
            .map(|mark| mark.platform)
            .filter(|platform| !platform.trim().is_empty())
            .unwrap_or_else(|| "windows".into());
        Ok((game, platform))
    }

    fn store_tokens(&self, game: &Game) -> Result<core_stores::StoreTokens> {
        let tokens = core_stores::load_tokens(&self.paths.data_dir, game.store).ok_or_else(|| {
            lapinux_core::Error::Other(format!(
                "compte {} non connecté : Réglages → Comptes",
                game.store.label()
            ))
        })?;
        self.valid_store_tokens(game.store, &tokens)
    }

    /// Renouvelle un jeton avant une lecture et l'enregistre aussitôt : Epic
    /// fait tourner le refresh token, le garder seulement en mémoire perdrait
    /// la session au renouvellement suivant.
    fn valid_store_tokens(
        &self,
        store: Store,
        tokens: &core_stores::StoreTokens,
    ) -> Result<core_stores::StoreTokens> {
        self.renew_store_tokens(store, tokens)
            .map_err(|error| lapinux_core::Error::Other(error.to_string()))
    }

    /// Renouvelle la session, sauf si l'utilisateur vient de déconnecter la
    /// boutique : une lecture partie avant la déconnexion ne doit pas recréer
    /// le fichier de jetons en arrivant.
    fn renew_store_tokens(
        &self,
        store: Store,
        tokens: &core_stores::StoreTokens,
    ) -> std::result::Result<core_stores::StoreTokens, lapinux_stores::Error> {
        if self.signed_out_marker(store).exists() {
            return Err(lapinux_stores::Error::Other(format!(
                "compte {} déconnecté",
                store.label()
            )));
        }
        lapinux_stores::valid_tokens_saved(&self.paths.data_dir, store, tokens)
    }

    /// Dossier local des sauvegardes : celui des réglages du jeu, sinon
    /// `sauvegardes/` à côté des fichiers du jeu.
    fn store_save_dir(game: &Game) -> std::path::PathBuf {
        game.settings
            .save_dir
            .as_deref()
            .map(str::trim)
            .filter(|dir| !dir.is_empty())
            .map(lapinux_core::paths::expand_tilde)
            .unwrap_or_else(|| game.install_dir().join("sauvegardes"))
    }

    /// Nom du dossier de sauvegarde, celui que GOG remet devant chaque fichier.
    fn save_dir_name(game: &Game, dir: &std::path::Path) -> String {
        dir.file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| safe_dirname(&game.name))
    }

    /// Espace de noms Epic (sandbox) : la bibliothèque du compte le publie, la
    /// fiche d'un jeu installé ne le garde pas.
    fn store_namespace(&self, store_id: &str) -> String {
        self.store
            .get_json::<StoresView>(STORES_KEY)
            .ok()
            .flatten()
            .map(|view| {
                view.libraries
                    .iter()
                    .flat_map(|library| library.entries.iter())
                    .find(|entry| entry.store_id == store_id)
                    .map(|entry| entry.namespace.clone())
                    .unwrap_or_default()
            })
            .unwrap_or_default()
    }

    /// Range les jeux de boutique déjà installés dans le rayon de leur
    /// launcher, à la racine de la bibliothèque : `<Boutique>/<jeu>`. Une
    /// installation faite avant ce rangement vivait sous `steamapps/common`,
    /// avec ou sans rayon ; elle est déplacée par un renommage (même système
    /// de fichiers), puis la fiche du jeu suit — dossier, exécutable — pour
    /// que lancement, désinstallation et fiche continuent de tomber juste.
    pub(crate) fn migrate_store_shelves(&self) {
        let games = match self.store.load_games() {
            Ok(games) => games,
            Err(error) => {
                tracing::warn!("rangement des jeux de boutique impossible : {error}");
                return;
            }
        };
        let mut moved = 0;
        for mut game in games {
            if !game.installed || game.origin != GameOrigin::Store || game.store == Store::Steam {
                continue;
            }
            let name = game.installdir.trim().to_string();
            // Nom qu'on ne sait pas manipuler sans risque : on ne touche à
            // rien. Un nom qui porte déjà un séparateur est déjà rangé (ou
            // l'a été sous `steamapps/common` par la version précédente).
            if name.is_empty() || name.contains('\\') {
                continue;
            }
            let shelf = shelf_name(game.store.label());
            let target_name = if name.contains('/') {
                name.clone()
            } else {
                format!("{shelf}/{name}")
            };
            let to = game.library.join(&target_name);
            // Une installation déjà présente à destination : on ne l'écrase
            // pas, on laisse la fiche telle quelle pour ne rien perdre.
            if to.exists() {
                continue;
            }
            // Sources possibles, de la plus récente à la plus ancienne :
            // sous `steamapps/common`, puis à la racine.
            let from = [
                game.library.join("steamapps").join("common").join(&name),
                game.library.join(&name),
            ]
            .into_iter()
            .find(|path| path.is_dir());
            let Some(from) = from else {
                continue;
            };
            if let Some(parent) = to.parent() {
                if let Err(error) = std::fs::create_dir_all(parent) {
                    tracing::warn!("rayon {} non créé : {error}", parent.display());
                    continue;
                }
            }
            if let Err(error) = std::fs::rename(&from, &to) {
                tracing::warn!(
                    "{} n'a pas pu rejoindre son rayon : {error}",
                    from.display()
                );
                continue;
            }
            // Le rayon vidé ne reste pas derrière : un dossier vide dans
            // `common` se lirait comme un jeu sans nom.
            if let Some(parent) = from.parent() {
                if parent.file_name().and_then(|name| name.to_str()) != Some("common") {
                    let _ = std::fs::remove_dir(parent);
                }
            }
            // Le chemin de l'exécutable est noté en absolu : il suit le
            // dossier, sinon la fiche proposerait un fichier disparu.
            if let Some(exe) = game.exe.as_ref() {
                if let Ok(rest) = exe.strip_prefix(&from) {
                    game.exe = Some(to.join(rest));
                }
            }
            game.installdir = target_name;
            if let Err(error) = self.store.save_game(&game) {
                tracing::warn!("fiche de {} non mise à jour : {error}", game.name);
                continue;
            }
            moved += 1;
            tracing::info!("{} rangé dans {shelf} : {}", game.name, to.display());
        }
        if moved > 0 {
            tracing::info!("{moved} jeu(x) de boutique rangé(s) par launcher");
        }
    }

    /// Jeu possédé chez une boutique, installé ou seulement dans sa
    /// bibliothèque : la fiche d'un jeu non installé a besoin des mêmes
    /// identifiants que celle d'un jeu installé.
    pub(crate) fn cached_store_entry(&self, appid: u32) -> Option<core_stores::StoreGame> {
        self.store
            .get_json::<StoresView>(STORES_KEY)
            .ok()
            .flatten()
            .unwrap_or_default()
            .libraries
            .iter()
            .flat_map(|library| library.entries.iter())
            .find(|entry| entry.appid == appid)
            .cloned()
    }

    /// Boutique, identifiant et espace de noms d'un jeu de boutique : la base
    /// locale quand il est installé, sa bibliothèque sinon.
    fn store_identity(&self, appid: u32) -> Result<(Store, String, String)> {
        if let Ok((game, _platform)) = self.store_game(appid) {
            let namespace = self.store_namespace(&game.store_id);
            return Ok((game.store, game.store_id, namespace));
        }
        self.cached_store_entry(appid)
            .map(|entry| (entry.store, entry.store_id, entry.namespace))
            .ok_or_else(|| lapinux_core::Error::Other("jeu de boutique inconnu".into()))
    }

    /// Description et visuels d'un jeu de boutique : lus chez la boutique,
    /// quand la fiche se déplie, plutôt que pour toute la bibliothèque.
    pub fn store_details(self: &Arc<Self>, appid: u32) -> Result<Envelope> {
        let shared = Arc::clone(self);
        self.cached_remote_read("boutique", appid, move || shared.read_store_details(appid))
    }

    /// Lecture réseau d'un résumé de boutique, sans passer par le cache.
    fn read_store_details(&self, appid: u32) -> Result<serde_json::Value> {
        let (store, store_id, namespace) = self.store_identity(appid)?;
        let tokens = core_stores::load_tokens(&self.paths.data_dir, store).ok_or_else(|| {
            lapinux_core::Error::Other(format!(
                "compte {} non connecté : Réglages → Comptes",
                store.label()
            ))
        })?;
        let tokens = self.valid_store_tokens(store, &tokens)?;
        let catalog_item = self
            .cached_store_entry(appid)
            .map(|entry| entry.catalog_item)
            .unwrap_or_default();
        // La langue choisie dans les réglages : la vitrine écrit sa
        // présentation dans celle-là.
        let locale = self.config_snapshot().general.lang;
        let details = lapinux_stores::details(
            store,
            &tokens,
            &store_id,
            &namespace,
            &catalog_item,
            &locale,
        )
        .map_err(|error| lapinux_core::Error::Other(error.to_string()))?;
        // La vitrine nomme parfois un studio que la liste de la bibliothèque
        // ignore : le crédit appris ici est gardé pour les filtres.
        self.learn_credits(appid, &details);
        serde_json::to_value(&details)
            .map_err(|error| lapinux_core::Error::Other(error.to_string()))
    }

    /// Retient l'éditeur et le développeur lus dans la fiche d'un jeu de
    /// boutique, quand ils en disent plus que la liste de la bibliothèque.
    fn learn_credits(&self, appid: u32, details: &core_stores::StoreDetails) {
        let developer = details.developer.trim();
        let publisher = details.publisher.trim();
        if developer.is_empty() && publisher.is_empty() {
            return;
        }
        let Ok(Some(mut view)) = self.store.get_json::<core_stores::StoresView>(STORES_KEY) else {
            return;
        };
        let mut changed = false;
        for library in &mut view.libraries {
            for entry in &mut library.entries {
                if entry.appid != appid {
                    continue;
                }
                // La liste d'une boutique recopie souvent l'éditeur chez le
                // développeur : un vrai studio venu de la vitrine le remplace.
                let unnamed =
                    !entry.publishers.is_empty() && entry.publishers == entry.developers;
                if !publisher.is_empty() && entry.publishers.is_empty() {
                    entry.publishers = vec![publisher.to_string()];
                    changed = true;
                }
                if !developer.is_empty() && (entry.developers.is_empty() || unnamed) {
                    entry.developers = vec![developer.to_string()];
                    changed = true;
                }
            }
        }
        if changed {
            self.store.set_json(STORES_KEY, &view).ok();
        }
    }

    /// Mises à jour publiées par les boutiques pour les jeux installés.
    ///
    /// Chaque jeu installé porte l'empreinte du plan qui l'a posé ; relire le
    /// plan courant dit si la boutique a publié un autre build. La lecture
    /// coûte un appel par jeu : elle se fait à la demande, bornée, et un jeu
    /// dont la boutique ne répond pas est simplement absent de la liste.
    pub fn store_updates(self: &Arc<Self>) -> Result<Envelope> {
        const MAX_GAMES: usize = 60;
        let games: Vec<Game> = self
            .store
            .load_games()
            .unwrap_or_default()
            .into_iter()
            .filter(|game| {
                game.store != Store::Steam && game.installed && !game.store_id.trim().is_empty()
            })
            .collect();
        let config = self.config_snapshot();
        let mut updates = Vec::new();
        for game in games.into_iter().take(MAX_GAMES) {
            let Some(mark) = core_stores::StoreMark::read(&game.install_dir()) else {
                continue;
            };
            // Une installation antérieure à l'empreinte ne peut pas être
            // comparée : elle sera reconnue après sa prochaine installation.
            if mark.build.trim().is_empty() {
                continue;
            }
            let Some(tokens) = core_stores::load_tokens(&self.paths.data_dir, game.store)
            else {
                continue;
            };
            let Ok(tokens) = self.valid_store_tokens(game.store, &tokens) else {
                continue;
            };
            let platform = if mark.platform.trim().is_empty() {
                "windows".to_string()
            } else {
                mark.platform.clone()
            };
            let Ok(plan) = lapinux_stores::install_plan(
                game.store,
                &tokens,
                &mark.store_id,
                &platform,
                &config.downloads.epic_cdn,
            ) else {
                continue;
            };
            let current = plan_fingerprint(&plan);
            // Les marques antérieures à l'identité stable d'Epic contiennent
            // l'empreinte d'une adresse CDN signée : elle changeait pour le
            // même build. On la rebase une fois, sans annoncer de mise à jour,
            // si la taille installée concorde encore avec le build publié.
            if mark.fingerprint_version < core_stores::StoreMark::FINGERPRINT_VERSION
                && !plan.build.trim().is_empty()
            {
                let same_size = plan.installed == 0
                    || game.size_on_disk.abs_diff(plan.installed) <= 8192;
                if same_size {
                    let mut migrated = mark.clone();
                    migrated.build = current;
                    migrated.fingerprint_version =
                        core_stores::StoreMark::FINGERPRINT_VERSION;
                    let _ = migrated.write(&game.install_dir());
                    continue;
                }
            }
            if current == mark.build {
                continue;
            }
            updates.push(serde_json::json!({
                "appid": game.appid,
                "name": game.name,
                "store": game.store,
                "store_id": game.store_id,
                "platform": platform,
                "size": plan.download_size(),
            }));
        }
        Ok(Envelope::ok(&updates))
    }

    /// Poids publié d'un jeu de boutique : le plan d'installation dit ce qu'il
    /// faut télécharger et ce que le jeu occupe une fois installé. La réponse
    /// est gardée en mémoire : sans elle, chaque ouverture de fiche repayait
    /// l'API de la boutique pour un chiffre qui ne change pas.
    pub fn store_size(self: &Arc<Self>, appid: u32) -> Result<Envelope> {
        let (store, store_id, _namespace) = self.store_identity(appid)?;
        let shared = Arc::clone(self);
        self.cached_remote_read("taille-boutique", appid, move || {
            shared.read_store_size(store, &store_id)
        })
    }

    /// Poids des contenus additionnels d'un jeu de boutique, lu chez elle.
    pub fn store_dlc_sizes(self: &Arc<Self>, appid: u32) -> Result<Envelope> {
        let shared = Arc::clone(self);
        self.cached_remote_read("contenus-boutique-v2", appid, move || {
            shared.read_store_dlc_sizes(appid)
        })
    }

    fn read_store_dlc_sizes(&self, appid: u32) -> Result<serde_json::Value> {
        let view = self
            .store
            .get_json::<StoresView>(STORES_KEY)?
            .unwrap_or_default();
        let game = view
            .libraries
            .iter()
            .flat_map(|library| library.entries.iter())
            .find(|entry| entry.appid == appid)
            .cloned()
            .ok_or_else(|| lapinux_core::Error::Other("jeu de boutique inconnu".into()))?;
        if game.store != Store::Epic {
            return Ok(serde_json::json!({ "contents": [] }));
        }
        let tokens = core_stores::load_tokens(&self.paths.data_dir, game.store).ok_or_else(|| {
            lapinux_core::Error::Other(format!(
                "compte {} non connecté : Réglages → Comptes",
                game.store.label()
            ))
        })?;
        let tokens = self.valid_store_tokens(game.store, &tokens)?;
        // La même version que le bouton d'installation proposerait.
        let platform = preferred(&game.platforms).unwrap_or_else(|| "windows".into());
        let mut contents = Vec::new();
        for content in &game.dlc {
            let build = lapinux_stores::dlc_build(
                game.store,
                &tokens,
                &content.namespace,
                &content.store_id,
                &content.app_name,
                &platform,
            );
            contents.push(serde_json::json!({
                "name": content.name,
                // L'identifiant de l'offre distingue les contenus entre eux,
                // même quand aucun nom d'application n'est publié.
                "item": content.store_id,
                "app": build
                    .as_ref()
                    .map(|build| build.0.clone())
                    .unwrap_or_else(|| content.app_name.clone()),
                "download": build.as_ref().map(|build| build.1).unwrap_or(0),
                "installed": build.as_ref().map(|build| build.2).unwrap_or(0),
            }));
        }
        Ok(serde_json::json!({ "contents": contents }))
    }

    /// Installe un jeu de boutique : prépare le plan, crée la tâche visible
    /// dans Téléchargements, puis télécharge et enregistre la fiche du jeu.
    pub fn store_install(
        self: &Arc<Self>,
        store: Store,
        store_id: String,
        platform: Option<String>,
    ) -> Result<Envelope> {
        let appid = core_stores::store_appid(store, &store_id);
        let tokens = core_stores::load_tokens(&self.paths.data_dir, store).ok_or_else(|| {
            lapinux_core::Error::Other(format!(
                "compte {} non connecté : Réglages → Comptes",
                store.label()
            ))
        })?;
        let tokens = self.valid_store_tokens(store, &tokens)?;
        let entry = self
            .store
            .get_json::<StoresView>(STORES_KEY)
            .ok()
            .flatten()
            .unwrap_or_default()
            .libraries
            .iter()
            .flat_map(|library| library.entries.iter())
            .find(|entry| entry.store == store && entry.store_id == store_id)
            .cloned();
        let platform = platform
            .filter(|platform| !platform.trim().is_empty())
            .or_else(|| entry.as_ref().and_then(|entry| preferred(&entry.platforms)))
            .unwrap_or_else(|| "linux".into());
        let config = self.config_snapshot();
        // La tâche apparaît tout de suite : Epic, GOG et itch mettent plusieurs
        // secondes à publier leurs fichiers, et la barre ne s'affichait
        // qu'après. Elle dit « préparation » entre-temps, puis reprend le plan
        // dès qu'il est connu.
        let job_id = format!(
            "{}-{}-{}",
            store.key(),
            appid,
            lapinux_core::util::now_unix()
        );
        let entry_name = entry.as_ref().map(|entry| entry.name.clone());
        let job = Job {
            id: job_id.clone(),
            kind: JobKind::InstallGame,
            label: entry_name
                .as_ref()
                .map(|name| format!("{name} ({store_id})"))
                .unwrap_or_else(|| store_id.clone()),
            title: entry_name.clone().unwrap_or_default(),
            appid: Some(appid),
            state: JobState::Queued,
            progress: 0.0,
            downloaded: 0,
            total: 0,
            network: 0,
            written: 0,
            server: String::new(),
            depot: 0,
            depots: 0,
            updating: false,
            dlc: Vec::new(),
            platform: Some(platform.clone()),
            message: "préparation".into(),
            backend: store.label().to_string(),
            started: lapinux_core::util::now_unix(),
            finished: None,
            report: None,
        };
        if let Ok(mut jobs) = self.jobs.lock() {
            jobs.push(job);
        }
        self.persist_jobs(true);
        let failed = |shared: &Arc<Self>, message: String| {
            shared.update_job(&job_id, |job| {
                job.state = JobState::Failed;
                job.finished = Some(lapinux_core::util::now_unix());
                job.message = message;
            });
            shared.persist_jobs(true);
        };
        let plan = match lapinux_stores::install_plan(
            store,
            &tokens,
            &store_id,
            &platform,
            &config.downloads.epic_cdn,
        ) {
            Ok(plan) if !plan.steps.is_empty() => plan,
            Ok(_) => {
                let message = format!(
                    "aucun téléchargement publié pour ce jeu en version {platform}"
                );
                failed(self, message.clone());
                return Err(lapinux_core::Error::Other(message));
            }
            Err(error) => {
                failed(self, error.to_string());
                return Err(lapinux_core::Error::Other(error.to_string()));
            }
        };
        let library_root = config.library_root();
        // Chaque boutique a son rayon à la racine de la bibliothèque : les
        // jeux de boutique ne se rangent pas dans `steamapps`, qui est la
        // structure de Steam.
        let shelf = shelf_name(store.label());
        let dirname = safe_dirname(&plan.installdir);
        let dest = library_root.join(&shelf).join(&dirname);
        // Le dossier inscrit dans la base comprend le rayon : `install_dir()`
        // le rejoint depuis la bibliothèque et retombe sur le bon chemin.
        let installdir = format!("{shelf}/{dirname}");
        self.update_job(&job_id, |job| {
            job.state = JobState::Running;
            job.label = format!("{} ({store_id})", plan.name);
            job.title = plan.name.clone();
            job.total = plan.download_size();
            job.server = plan_server(&plan);
            // Seule une installation déjà terminée par Lapinux est une mise à
            // jour : un dossier laissé par un essai interrompu (ou par le
            // client de la boutique) ne fait pas de la pose une reprise.
            job.updating = core_stores::StoreMark::read(&dest).is_some();
            job.message = "téléchargement".into();
        });
        self.persist_jobs(true);
        let cancel = Arc::new(AtomicBool::new(false));
        if let Ok(mut map) = self.cancel.lock() {
            map.insert(job_id.clone(), Arc::clone(&cancel));
        }
        // Le drapeau de pause est celui que l'interface bascule : sans lui,
        // une tâche de boutique répondait « tâche inconnue » au bouton pause.
        let pause = Arc::new(AtomicBool::new(false));
        if let Ok(mut map) = self.pause.lock() {
            map.insert(job_id.clone(), Arc::clone(&pause));
        }

        let shared = Arc::clone(self);
        let thread_job = job_id.clone();
        let cache_dir = self.paths.cache_dir.join("stores");
        let identity = core_stores::StoreMark {
            store,
            store_id: plan.store_id.clone(),
            platform: platform.clone(),
            installdir: installdir.clone(),
            name: plan.name.clone(),
            build: plan_fingerprint(&plan),
            fingerprint_version: core_stores::StoreMark::FINGERPRINT_VERSION,
        };
        let name = plan.name.clone();
        let total = plan.download_size();
        let platform_label = platform.clone();
        // Le plafond de débit du store et le plafond global, réglés dans les
        // paramètres : les installations de boutique passaient au travers et
        // téléchargeaient à pleine vitesse. Les limiteurs vivent dans le
        // partage : leur valeur est relue à chaque tour, un réglage modifié
        // s'applique donc au transfert en cours.
        let limit = shared.throttle.limit(store.key());
        std::thread::spawn(move || {
            let mut last = 0u64;
            // Référence du bridage : instant, octets déjà reçus et plafond en
            // vigueur. Elle se remet à zéro quand le plafond change, sinon un
            // réglage appliqué en cours de route ferait « payer » tout ce qui
            // était déjà descendu. `None` : rien à brider.
            let mut bridle: Option<(std::time::Instant, u64, u64)> = None;
            let cap_bytes_per_sec = || {
                let mut cap = 0u64;
                for limiter in [limit.global.as_ref(), limit.store.as_ref()]
                    .into_iter()
                    .flatten()
                {
                    let rate = limiter.rate();
                    if rate > 0 {
                        cap = if cap == 0 { rate } else { cap.min(rate) };
                    }
                }
                cap
            };
            let mut tick = |done: u64, size: u64| -> bool {
                // En pause, le moteur de la boutique attend ici : le fichier en
                // cours se termine, rien n'est perdu, et la reprise repart du
                // même point. L'annulation reste obéie pendant l'attente.
                while pause.load(Ordering::Relaxed) {
                    if cancel.load(Ordering::Relaxed) {
                        return false;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(150));
                }
                if cancel.load(Ordering::Relaxed) {
                    return false;
                }
                // Bridage : le débit moyen ne doit pas dépasser le plafond. Le
                // moteur appelle ce retour souvent ; on dort juste ce qu'il
                // faut, ce qui ralentit aussi la lecture en cours.
                let cap = cap_bytes_per_sec();
                if cap == 0 {
                    bridle = None;
                } else {
                    let (anchor, base) = match bridle {
                        Some((anchor, base, current)) if current == cap => (anchor, base),
                        _ => {
                            let now = std::time::Instant::now();
                            bridle = Some((now, done, cap));
                            (now, done)
                        }
                    };
                    // La dette se paie en entier : la plafonner par morceau
                    // laissait filer 3 à 4 fois le débit demandé, un chunk
                    // arrivant déjà plus gros que la tranche.
                    loop {
                        let owed = (done.saturating_sub(base)) as f64
                            - cap as f64 * anchor.elapsed().as_secs_f64();
                        if owed <= 0.0 {
                            break;
                        }
                        if cancel.load(Ordering::Relaxed) {
                            return false;
                        }
                        while pause.load(Ordering::Relaxed) {
                            if cancel.load(Ordering::Relaxed) {
                                return false;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(150));
                        }
                        let slice = (owed / cap as f64).min(0.05);
                        std::thread::sleep(std::time::Duration::from_secs_f64(slice));
                    }
                }
                // Une mise à jour tous les 4 Mio suffit à l'affichage, et
                // épargne la base et l'interface sur un gros jeu. Bridé, on
                // rapporte plus souvent pour que la barre avance régulièrement.
                let report_step = if cap > 0 {
                    512 * 1024
                } else {
                    4 * 1024 * 1024
                };
                if done.saturating_sub(last) < report_step && done != size {
                    return true;
                }
                last = done;
                shared.update_job(&thread_job, |job| {
                    job.downloaded = done;
                    job.network = done;
                    job.written = done;
                    if size > 0 {
                        job.total = size;
                        job.progress = (done as f32 / size as f32).clamp(0.0, 1.0);
                    }
                    job.message = "téléchargement et extraction".into();
                });
                true
            };
            let result = lapinux_stores::install(&tokens, &plan, &cache_dir, &dest, &mut tick);
            match result {
                Ok(()) => {
                    if let Err(error) = identity.write(&dest) {
                        tracing::warn!(
                            "marque de boutique non écrite dans {} : {error}",
                            dest.display()
                        );
                    }
                    // Le jeu entre dans la bibliothèque avec son exécutable :
                    // la fiche et le lancement le traitent comme les autres.
                    let mut game = shared.store.game(appid).ok().flatten().unwrap_or(Game {
                        appid,
                        ..Default::default()
                    });
                    game.name = name.clone();
                    game.installdir = installdir.clone();
                    game.library = library_root.clone();
                    game.origin = GameOrigin::Store;
                    game.store = store;
                    game.store_id = plan.store_id.clone();
                    game.size_on_disk = dir_size(&dest);
                    game.state_flags = 4;
                    game.last_updated = lapinux_core::util::now_unix();
                    game.build_id.clear();
                    game.installed = true;
                    game.prefix = None;
                    game.exe = find_executable(&dest, &plan.installdir);
                    game.native = platform != "windows";
                    game.drm = Drm::None;
                    if let Err(error) = shared.store.save_game(&game) {
                        tracing::warn!("jeu {} non enregistré : {error}", game.appid);
                    }
                    let mut cloud = String::new();
                    let cloud_enabled = shared
                        .config_snapshot()
                        .downloads
                        .cloud_saves_after_install;
                    if store.capabilities().cloud
                        && game.settings.save_dir.is_some()
                        && game.settings.cloud_sync_enabled()
                        && cloud_enabled
                    {
                        shared.update_job(&thread_job, |job| {
                            job.message = "sauvegardes cloud".into();
                        });
                        match shared.store_cloud_pull_auto(appid) {
                            Ok((files, bytes)) if files > 0 => {
                                tracing::info!(
                                    "cloud {} : {files} sauvegarde(s) reprise(s) après l'installation de {appid} ({bytes} octets)",
                                    store.label()
                                );
                                cloud = format!(" · {files} sauvegarde(s) cloud");
                            }
                            Ok(_) => {}
                            Err(error) => tracing::debug!(
                                "cloud {} : rien repris après l'installation de {appid} ({error})",
                                store.label()
                            ),
                        }
                    }
                    shared.update_job(&thread_job, |job| {
                        job.state = JobState::Done;
                        job.progress = 1.0;
                        if job.total == 0 && total > 0 {
                            job.total = total;
                        }
                        job.finished = Some(lapinux_core::util::now_unix());
                        job.message = format!(
                            "{}{cloud}",
                            if game.exe.is_some() {
                                "installé"
                            } else {
                                "installé · exécutable à choisir dans les réglages du jeu"
                            }
                        );
                    });
                    shared.stores_view(true, &[]).ok();
                }
                Err(error) => {
                    let cancelled = cancel.load(Ordering::Relaxed);
                    shared.update_job(&thread_job, |job| {
                        job.state = if cancelled {
                            JobState::Cancelled
                        } else {
                            JobState::Failed
                        };
                        job.finished = Some(lapinux_core::util::now_unix());
                        job.message = error.to_string();
                    });
                }
            }
            shared.persist_jobs(true);
            if let Ok(mut jobs) = shared.jobs.lock() {
                if jobs.len() > crate::JOBS_KEEP {
                    let excess = jobs.len() - crate::JOBS_KEEP;
                    jobs.drain(0..excess);
                }
            }
            if let Ok(mut map) = shared.cancel.lock() {
                map.remove(&thread_job);
            }
            if let Ok(mut map) = shared.pause.lock() {
                map.remove(&thread_job);
            }
        });

        Ok(Envelope::ok(&serde_json::json!({
            "job": job_id,
            "appid": appid,
            "platform": platform_label,
        })))
    }
}

impl Shared {
    /// Marque laissée par une déconnexion : une lecture partie juste avant ne
    /// doit pas recréer le fichier de jetons en arrivant.
    fn signed_out_marker(&self, store: Store) -> PathBuf {
        core_stores::stores_dir(&self.paths.data_dir).join(format!(
            "{}.signed-out",
            store.key()
        ))
    }

    /// Lit les comptes et les bibliothèques des trois boutiques externes.
    ///
    /// Sans `refresh`, seule la vue en cache est rendue : l'ouverture des
    /// réglages ne doit jamais attendre le réseau. Avec `refresh`, les
    /// boutiques demandées sont relues ; les autres gardent leur cache.
    pub fn stores_view(&self, refresh: bool, only: &[Store]) -> Result<Envelope> {
        let mut view = self
            .store
            .get_json::<StoresView>(STORES_KEY)
            .ok()
            .flatten()
            .unwrap_or_default();
        for store in Store::EXTERNAL {
            let tokens = core_stores::load_tokens(&self.paths.data_dir, store);
            let mut account = view
                .accounts
                .iter()
                .find(|account| account.store == store)
                .cloned()
                .unwrap_or_else(|| StoreAccount::disconnected(store));
            account.capabilities = store.capabilities();
            if let Ok(link) = lapinux_stores::login_link(store) {
                account.login_url = link.url;
                account.login_hint = link.hint;
            }
            if let Some(tokens) = &tokens {
                account.connected = true;
                account.expires_at = tokens.expires_at;
                if !tokens.account.trim().is_empty() {
                    account.account = tokens.account.clone();
                }
                account.note.clear();
            } else {
                account.connected = false;
                account.account.clear();
                account.expires_at = 0;
            }
            if let Some(slot) = view.accounts.iter_mut().find(|slot| slot.store == store) {
                *slot = account;
            } else {
                view.accounts.push(account);
            }
            if !view.libraries.iter().any(|library| library.store == store) {
                view.libraries.push(StoreLibrary {
                    store,
                    ..Default::default()
                });
            }
        }
        view.accounts.retain(|account| account.store != Store::Steam);
        view.libraries.retain(|library| library.store != Store::Steam);

        if refresh {
            let selected: Vec<Store> = if only.is_empty() {
                Store::EXTERNAL.to_vec()
            } else {
                only.iter()
                    .copied()
                    .filter(|store| *store != Store::Steam)
                    .collect()
            };
            let installed: HashSet<(Store, String)> = self
                .store
                .load_games()
                .unwrap_or_default()
                .into_iter()
                .filter(|game| game.installed)
                .map(|game| (game.store, game.store_id))
                .collect();
            for store in selected {
                let Some(tokens) =
                    core_stores::load_tokens(&self.paths.data_dir, store)
                else {
                    if let Some(library) =
                        view.libraries.iter_mut().find(|library| library.store == store)
                    {
                        library.store = store;
                        library.fetched = 0;
                        library.entries.clear();
                        library.error = Some(format!("compte {} non connecté", store.label()));
                    }
                    continue;
                };
                let tokens = match self.valid_store_tokens(store, &tokens) {
                    Ok(tokens) => tokens,
                    Err(error) => {
                        if let Some(library) =
                            view.libraries.iter_mut().find(|library| library.store == store)
                        {
                            library.error = Some(error.to_string());
                        }
                        continue;
                    }
                };
                let mut entries = match lapinux_stores::library(store, &tokens) {
                    Ok(entries) => entries,
                    Err(error) => {
                        if let Some(library) =
                            view.libraries.iter_mut().find(|library| library.store == store)
                        {
                            library.error = Some(error.to_string());
                        }
                        continue;
                    }
                };
                for entry in &mut entries {
                    entry.store = store;
                    entry.appid = core_stores::store_appid(store, &entry.store_id);
                    entry.installed = installed.contains(&(store, entry.store_id.clone()));
                }
                dedupe_entries(&mut entries);
                let library = view
                    .libraries
                    .iter_mut()
                    .find(|library| library.store == store)
                    .expect("bibliothèque créée plus haut");
                library.store = store;
                library.account = tokens.account.clone();
                library.fetched = lapinux_core::util::now_unix();
                library.entries = entries;
                library.error = None;
                if let Some(account) =
                    view.accounts.iter_mut().find(|account| account.store == store)
                {
                    account.connected = true;
                    account.expires_at = tokens.expires_at;
                    if !tokens.account.trim().is_empty() {
                        account.account = tokens.account.clone();
                    }
                    account.note.clear();
                }
            }
        }
        self.store.set_json(STORES_KEY, &view).ok();
        Ok(Envelope::ok(&view))
    }

    /// Termine une connexion par code d'autorisation (Epic, GOG) ou par clé
    /// d'API (itch), puis rend la vue des comptes : l'écran qui a envoyé le
    /// code l'attend dans cette forme.
    pub fn store_login(
        &self,
        store: Store,
        token: String,
        username: String,
    ) -> Result<Envelope> {
        if store == Store::Steam {
            return Err(lapinux_core::Error::Other(
                "Steam a sa propre connexion".into(),
            ));
        }
        let code = token.trim();
        if code.is_empty() {
            return Err(lapinux_core::Error::Other(
                "collez le code d'autorisation ou la clé d'API".into(),
            ));
        }
        let mut tokens = if store == Store::Itch {
            lapinux_stores::login_key(store, code)
        } else {
            lapinux_stores::login_code(store, code)
        }
        .map_err(|error| lapinux_core::Error::Other(error.to_string()))?;
        let username = username.trim();
        if !username.is_empty() && tokens.account.trim().is_empty() {
            tokens.account = username.to_string();
        }
        core_stores::save_tokens(&self.paths.data_dir, &tokens)
            .map_err(|error| lapinux_core::Error::Other(error.to_string()))?;
        let _ = std::fs::remove_file(self.signed_out_marker(store));
        self.stores_view(true, &[store])
    }

    /// Retire le compte d'une boutique, efface ses jetons et sa bibliothèque
    /// en cache, puis rend la vue avec le compte revenu à « non connecté ».
    pub fn store_logout(&self, store: Store) -> Result<Envelope> {
        if store == Store::Steam {
            return Err(lapinux_core::Error::Other(
                "Steam a sa propre déconnexion".into(),
            ));
        }
        core_stores::clear_tokens(&self.paths.data_dir, store)
            .map_err(|error| lapinux_core::Error::Other(error.to_string()))?;
        let marker = self.signed_out_marker(store);
        if let Some(parent) = marker.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| lapinux_core::Error::Other(error.to_string()))?;
        }
        std::fs::write(&marker, lapinux_core::util::now_unix().to_string())
            .map_err(|error| lapinux_core::Error::Other(error.to_string()))?;
        if let Ok(Some(mut view)) = self.store.get_json::<StoresView>(STORES_KEY) {
            view.libraries.retain(|library| library.store != store);
            view.accounts.retain(|account| account.store != store);
            let _ = self.store.set_json(STORES_KEY, &view);
        }
        self.stores_view(false, &[])
    }

    /// Poids à télécharger et poids installé publiés par la boutique pour un
    /// jeu : la fiche l'affiche avant de lancer l'installation.
    fn read_store_size(&self, store: Store, store_id: &str) -> Result<serde_json::Value> {
        let tokens = core_stores::load_tokens(&self.paths.data_dir, store).ok_or_else(|| {
            lapinux_core::Error::Other(format!(
                "compte {} non connecté : Réglages → Comptes",
                store.label()
            ))
        })?;
        let tokens = self.valid_store_tokens(store, &tokens)?;
        let entry = self
            .store
            .get_json::<StoresView>(STORES_KEY)
            .ok()
            .flatten()
            .unwrap_or_default()
            .libraries
            .iter()
            .flat_map(|library| library.entries.iter())
            .find(|entry| entry.store == store && entry.store_id == store_id)
            .cloned();
        let platform = entry
            .as_ref()
            .and_then(|entry| preferred(&entry.platforms))
            .unwrap_or_else(|| "linux".into());
        let config = self.config_snapshot();
        let plan = lapinux_stores::install_plan(
            store,
            &tokens,
            store_id,
            &platform,
            &config.downloads.epic_cdn,
        )
        .map_err(|error| lapinux_core::Error::Other(error.to_string()))?;
        Ok(serde_json::json!({
            "size": plan.download_size(),
            "installed": plan.installed,
        }))
    }

    /// Jeux offerts en ce moment par Epic : les offres déjà rattachées au
    /// compte sont marquées pour que la page ne propose pas de les reprendre.
    pub fn store_free_games(&self) -> Result<Envelope> {
        let locale = self.config_snapshot().general.lang;
        let mut games = lapinux_stores::free_games(&locale)
            .map_err(|error| lapinux_core::Error::Other(error.to_string()))?;
        if let Some(tokens) = core_stores::load_tokens(&self.paths.data_dir, Store::Epic) {
            if let Ok(tokens) = self.valid_store_tokens(Store::Epic, &tokens) {
                let _ = lapinux_stores::mark_owned(&tokens, &mut games);
            }
        }
        Ok(Envelope::ok(&games))
    }

    /// Rattache au compte Epic les offres choisies et rend le verdict par
    /// offre : une offre refusée ne doit pas faire échouer les autres.
    pub fn store_claim_free_games(&self, ids: Vec<String>) -> Result<Envelope> {
        let tokens = core_stores::load_tokens(&self.paths.data_dir, Store::Epic).ok_or_else(|| {
            lapinux_core::Error::Other(
                "compte Epic Games non connecté : Réglages → Comptes".into(),
            )
        })?;
        let tokens = self.valid_store_tokens(Store::Epic, &tokens)?;
        let locale = self.config_snapshot().general.lang;
        let games = lapinux_stores::free_games(&locale)
            .map_err(|error| lapinux_core::Error::Other(error.to_string()))?;
        let selected: Vec<_> = games
            .into_iter()
            .filter(|game| ids.contains(&game.id))
            .collect();
        if selected.is_empty() {
            return Err(lapinux_core::Error::Other(
                "aucune offre gratuite à récupérer".into(),
            ));
        }
        let outcomes = lapinux_stores::claim_free(&tokens, &selected, &locale)
            .map_err(|error| lapinux_core::Error::Other(error.to_string()))?;
        Ok(Envelope::ok(&outcomes))
    }

    /// Succès d'un jeu de boutique, lus sans cache : la surveillance d'une
    /// partie doit voir un déblocage immédiatement.
    pub(crate) fn read_store_achievements_now(
        &self,
        appid: u32,
    ) -> Result<SteamAchievements> {
        let (store, store_id, namespace) = self.store_identity(appid)?;
        let tokens = core_stores::load_tokens(&self.paths.data_dir, store).ok_or_else(|| {
            lapinux_core::Error::Other(format!(
                "compte {} non connecté : Réglages → Comptes",
                store.label()
            ))
        })?;
        let tokens = self.valid_store_tokens(store, &tokens)?;
        let achievements = lapinux_stores::achievements(store, &tokens, &store_id, &namespace)
            .map_err(|error| lapinux_core::Error::Other(error.to_string()))?;
        let items = achievements
            .into_iter()
            .map(|achievement| SteamAchievement {
                api_name: achievement.key,
                name: achievement.name,
                description: achievement.description,
                icon: achievement.icon,
                hidden: achievement.hidden,
                achieved: achievement.unlocked,
                unlock_time: achievement.unlock_time,
                percent: achievement.percent,
            })
            .collect();
        Ok(SteamAchievements {
            appid,
            fetched: lapinux_core::util::now_unix(),
            items,
            play_minutes: None,
            notes: Vec::new(),
        })
    }

    /// Succès de boutique avec le cache des fiches : l'ouverture d'un jeu
    /// n'attend pas l'API à chaque repeint.
    pub fn store_achievements(self: &Arc<Self>, appid: u32) -> Result<Envelope> {
        let shared = Arc::clone(self);
        self.cached_remote_read("succès-boutique", appid, move || {
            let achievements = shared.read_store_achievements_now(appid)?;
            serde_json::to_value(&achievements).map_err(|error| {
                lapinux_core::Error::Other(format!("succès illisibles : {error}"))
            })
        })
    }

    /// Prépare tout ce qu'il faut pour lire, télécharger ou envoyer les
    /// sauvegardes d'un jeu de boutique.
    fn store_cloud_context(&self, appid: u32) -> Result<CloudContext> {
        let (game, platform) = self.store_game(appid)?;
        let store = game.store;
        let tokens = core_stores::load_tokens(&self.paths.data_dir, store).ok_or_else(|| {
            lapinux_core::Error::Other(format!(
                "compte {} non connecté : Réglages → Comptes",
                store.label()
            ))
        })?;
        let tokens = self.valid_store_tokens(store, &tokens)?;
        let dir = Self::store_save_dir(&game);
        let dir_name = Self::save_dir_name(&game, &dir);
        let files = lapinux_stores::cloud_list(
            store,
            &tokens,
            &game.store_id,
            &platform,
            &dir_name,
        )
        .map_err(|error| lapinux_core::Error::Other(error.to_string()))?;
        Ok(CloudContext {
            appid,
            store,
            store_id: game.store_id,
            platform,
            dir,
            dir_name,
            tokens,
            files,
        })
    }

    /// Relevé du cloud d'un jeu de boutique pour la fiche : les fichiers du
    /// compte, ceux déjà présents localement et le dossier où ils vivent.
    pub fn store_cloud(&self, appid: u32) -> Result<Envelope> {
        let mut report = CloudReport::default();
        match self.store_cloud_context(appid) {
            Ok(context) => {
                report.saves.files = context
                    .files
                    .iter()
                    .map(|file| {
                        let local = safe_relative(&file.name)
                            .and_then(|relative| local_modified(&context.dir.join(relative)));
                        CloudFile {
                            name: file.name.clone(),
                            size: file.size,
                            modified: file.modified,
                            local,
                            ..Default::default()
                        }
                    })
                    .collect();
                report.local_dir = Some(context.dir.display().to_string());
                report.local_present = report
                    .saves
                    .files
                    .iter()
                    .filter(|file| file.local.is_some())
                    .count();
            }
            Err(error) => report.error = Some(error.to_string()),
        }
        report.syncs = crate::cloud_journal::history(&self.paths, appid);
        Ok(Envelope::ok(&report))
    }

    /// Télécharge toutes les sauvegardes du compte dans le dossier du jeu.
    pub fn store_cloud_restore(&self, appid: u32) -> Result<Envelope> {
        self.ensure_store_game_closed(appid, "récupérer")?;
        let context = self.store_cloud_context(appid)?;
        let (written, bytes) = self
            .pull_store_cloud(&context, false)
            .map_err(|error| lapinux_core::Error::Other(format!("sauvegardes cloud : {error}")))?;
        Ok(Envelope::ok(&serde_json::json!({
            "files": written,
            "bytes": bytes,
        })))
    }

    /// Envoie vers le cloud les sauvegardes que la partie a écrites.
    pub fn store_cloud_upload(&self, appid: u32) -> Result<Envelope> {
        self.ensure_store_game_closed(appid, "envoyer")?;
        let context = self.store_cloud_context(appid)?;
        let (files, bytes) = self
            .push_store_cloud(&context)
            .map_err(|error| lapinux_core::Error::Other(format!("sauvegardes cloud : {error}")))?;
        Ok(Envelope::ok(&serde_json::json!({
            "files": files,
            "bytes": bytes,
        })))
    }

    /// Reprise automatique avant une partie : seuls les fichiers du cloud plus
    /// récents que leur copie locale sont ramenés.
    pub fn store_cloud_pull_auto(
        &self,
        appid: u32,
    ) -> std::result::Result<(usize, u64), String> {
        let game = self
            .store
            .game(appid)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "jeu introuvable".to_string())?;
        if !game.settings.cloud_sync_enabled() {
            return Ok((0, 0));
        }
        let context = self
            .store_cloud_context(appid)
            .map_err(|error| error.to_string())?;
        let result = self.pull_store_cloud(&context, true);
        if matches!(&result, Ok((files, _)) if *files > 0) {
            if let Ok(mut cache) = self.game_reads.lock() {
                cache.forget("cloud", appid);
            }
            self.forget_remote_sheet("cloud", appid);
        }
        result
    }

    /// Envoi automatique après une partie : seuls les fichiers modifiés
    /// depuis le relevé du cloud repartent.
    pub fn store_cloud_push_auto(
        &self,
        appid: u32,
    ) -> std::result::Result<(usize, u64, bool), String> {
        let game = self
            .store
            .game(appid)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "jeu introuvable".to_string())?;
        if !game.settings.cloud_sync_enabled() {
            return Ok((0, 0, false));
        }
        let context = self
            .store_cloud_context(appid)
            .map_err(|error| error.to_string())?;
        let present = !context.files.is_empty();
        let (files, bytes) = self.push_store_cloud(&context)?;
        if files > 0 {
            if let Ok(mut cache) = self.game_reads.lock() {
                cache.forget("cloud", appid);
            }
            self.forget_remote_sheet("cloud", appid);
        }
        Ok((files, bytes, present))
    }

    /// Refuse une opération cloud pendant que le jeu tourne : ses sauvegardes
    /// sont justement en cours d'écriture.
    fn ensure_store_game_closed(&self, appid: u32, action: &str) -> Result<()> {
        let running = self
            .session
            .lock()
            .map(|session| {
                session
                    .as_ref()
                    .map(|state| state.info.appid == appid)
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        if running {
            return Err(lapinux_core::Error::Other(format!(
                "quittez le jeu avant de {action} ses sauvegardes"
            )));
        }
        Ok(())
    }

    fn pull_store_cloud(
        &self,
        context: &CloudContext,
        only_newer: bool,
    ) -> std::result::Result<(usize, u64), String> {
        let mut written = 0usize;
        let mut bytes = 0u64;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(25);
        for file in &context.files {
            if only_newer && std::time::Instant::now() > deadline {
                break;
            }
            let Some(relative) = safe_relative(&file.name) else {
                tracing::warn!("cloud : {} ignoré, chemin invalide", file.name);
                continue;
            };
            let dest = context.dir.join(relative);
            if only_newer && file.modified > 0 {
                if let Some(local) = local_modified(&dest) {
                    if local >= file.modified {
                        continue;
                    }
                }
            }
            let size = lapinux_stores::cloud_download(
                context.store,
                &context.tokens,
                &context.store_id,
                &context.platform,
                &context.dir_name,
                file,
                &dest,
            )
            .map_err(|error| error.to_string())?;
            written += 1;
            bytes += size;
        }
        if written > 0 {
            crate::cloud_journal::record(
                &self.paths,
                context.appid,
                lapinux_core::model::CloudSyncEntry {
                    at: lapinux_core::util::now_unix(),
                    direction: lapinux_core::model::CloudSyncDirection::Download,
                    machine: format!("{} Cloud", context.store.label()),
                    files: written,
                    bytes,
                },
            );
        }
        Ok((written, bytes))
    }

    fn push_store_cloud(
        &self,
        context: &CloudContext,
    ) -> std::result::Result<(usize, u64), String> {
        let mut local_files = Vec::new();
        collect_saves(&context.dir, &context.dir, &mut local_files);
        let mut sent = 0usize;
        let mut bytes = 0u64;
        for (name, path) in local_files {
            let Some(modified) = local_modified(&path) else {
                continue;
            };
            if let Some(remote) = context.files.iter().find(|file| file.name == name) {
                if remote.modified > 0 && modified <= remote.modified {
                    continue;
                }
            }
            let data = std::fs::read(&path).map_err(|error| error.to_string())?;
            let remote = lapinux_stores::RemoteFile {
                name: name.clone(),
                size: data.len() as u64,
                modified,
                full_name: if context.dir_name.is_empty() {
                    name
                } else {
                    format!("{}/{name}", context.dir_name)
                },
                ..Default::default()
            };
            let size = lapinux_stores::cloud_upload(
                context.store,
                &context.tokens,
                &context.store_id,
                &context.platform,
                &context.dir_name,
                &remote,
                data,
                modified,
            )
            .map_err(|error| error.to_string())?;
            sent += 1;
            bytes += size;
        }
        if sent > 0 {
            crate::cloud_journal::record(
                &self.paths,
                context.appid,
                lapinux_core::model::CloudSyncEntry {
                    at: lapinux_core::util::now_unix(),
                    direction: lapinux_core::model::CloudSyncDirection::Upload,
                    machine: crate::cloud_journal::machine_name(),
                    files: sent,
                    bytes,
                },
            );
        }
        Ok((sent, bytes))
    }

    /// Prépare en arrière-plan les fiches des jeux de boutique récemment
    /// joués : le premier clic les trouve déjà sur disque.
    pub(crate) fn start_store_preload(self: &Arc<Self>) {
        let shared = Arc::clone(self);
        std::thread::spawn(move || {
            let mut games = shared.store.load_games().unwrap_or_default();
            games.retain(|game| game.store != Store::Steam && game.installed);
            games.sort_by(|left, right| {
                right
                    .last_played
                    .unwrap_or(0)
                    .cmp(&left.last_played.unwrap_or(0))
            });
            for game in games.into_iter().take(8) {
                let _ = shared.store_details(game.appid);
            }
        });
    }
}

/// Dernière modification d'un fichier, en secondes Unix.
fn local_modified(path: &Path) -> Option<i64> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .map(unix_seconds)
}

/// Tout ce qu'il faut pour synchroniser un jeu de boutique.
struct CloudContext {
    appid: u32,
    store: Store,
    store_id: String,
    platform: String,
    dir: PathBuf,
    dir_name: String,
    tokens: core_stores::StoreTokens,
    files: Vec<lapinux_stores::RemoteFile>,
}

/// Secondes Unix d'une date de fichier.
fn unix_seconds(time: std::time::SystemTime) -> i64 {
    time.duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

/// Chemin venu de la boutique, ramené sous le dossier de sauvegarde : un nom
/// absolu ou remontant (`../`) est refusé.
fn safe_relative(name: &str) -> Option<std::path::PathBuf> {
    let mut out = std::path::PathBuf::new();
    for component in std::path::Path::new(name).components() {
        match component {
            std::path::Component::Normal(part) => out.push(part),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// Fichiers d'un dossier de sauvegarde, avec leur chemin relatif en `/`.
fn collect_saves(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(String, std::path::PathBuf)>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            collect_saves(root, &path, out);
            continue;
        }
        if !kind.is_file() {
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let name = relative
            .components()
            .filter_map(|component| match component {
                std::path::Component::Normal(part) => Some(part.to_string_lossy().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/");
        if !name.is_empty() {
            out.push((name, path));
        }
    }
}

/// Version préférée quand l'utilisateur n'en choisit pas : Linux d'abord, puis
/// Windows sous Proton, puis macOS — le même ordre que les menus de version.
fn preferred(platforms: &[String]) -> Option<String> {
    ["linux", "windows", "macos"]
        .into_iter()
        .find(|wanted| platforms.iter().any(|platform| platform == wanted))
        .map(str::to_string)
}

/// Une ligne par jeu dans une bibliothèque de boutique : un même identifiant
/// ne revient pas, et deux titres qui ne diffèrent que par la ponctuation ou
/// les marques déposées font une seule ligne. La copie installée gagne : son
/// identifiant est celui que la fiche lance, règle et désinstalle.
fn dedupe_entries(entries: &mut Vec<core_stores::StoreGame>) {
    let mut seen: HashSet<u32> = HashSet::new();
    entries.retain(|entry| seen.insert(entry.appid));
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut unique: Vec<core_stores::StoreGame> = Vec::with_capacity(entries.len());
    for entry in entries.drain(..) {
        let key = core_stores::title_key(&entry.name);
        let Some(position) = index.get(&key).copied() else {
            index.insert(key, unique.len());
            unique.push(entry);
            continue;
        };
        let kept = &mut unique[position];
        if entry.installed && !kept.installed {
            let cover = std::mem::take(&mut kept.cover);
            let catalog = std::mem::take(&mut kept.catalog_item);
            *kept = entry;
            if kept.cover.is_empty() {
                kept.cover = cover;
            }
            if kept.catalog_item.is_empty() {
                kept.catalog_item = catalog;
            }
            continue;
        }
        if kept.cover.is_empty() {
            kept.cover = entry.cover;
        }
        if kept.catalog_item.is_empty() {
            kept.catalog_item = entry.catalog_item;
        }
        // Un jeu l'emporte sur un outil de même titre.
        if kept.tool && !entry.tool {
            kept.tool = false;
        }
    }
    *entries = unique;
}

/// Nom de dossier sans séparateur ni retour arrière : un `installdir` venu de
/// la boutique sert à construire un chemin.
/// Empreinte du build d'un plan : les adresses de ses étapes, dans l'ordre.
/// Deux builds publiés n'ont pas les mêmes ; réinstaller le même jeu ne change
/// donc pas la marque, et une nouvelle publication se voit tout de suite.
/// Hôte CDN qui sert réellement le plan : le premier lien publié par la
/// boutique (manifeste Epic, installeur GOG, archive itch). La fiche de
/// téléchargement annonçait « Epic Games » comme serveur, ce qui ne dit rien
/// du chemin réseau emprunté.
fn plan_server(plan: &lapinux_stores::InstallPlan) -> String {
    for step in &plan.steps {
        let url = match step {
            lapinux_stores::Step::Download { url, .. }
            | lapinux_stores::Step::EpicBuild { url, .. } => url,
            _ => continue,
        };
        let host = url
            .split("://")
            .nth(1)
            .and_then(|rest| rest.split(['/', '?']).next())
            .unwrap_or_default();
        if !host.is_empty() {
            return host.to_string();
        }
    }
    plan.store.label().to_string()
}

fn plan_fingerprint(plan: &lapinux_stores::InstallPlan) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    // Une identité de build fournie par la boutique (Epic) ne dépend ni du
    // miroir CDN ni de son jeton : c'est elle qui décide d'une mise à jour.
    if !plan.build.trim().is_empty() {
        for text in [
            plan.store.key(),
            plan.store_id.as_str(),
            plan.platform.as_str(),
            plan.build.trim(),
        ] {
            for byte in text.as_bytes() {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        return format!("{hash:016x}");
    }
    for step in &plan.steps {
        let text = match step {
            lapinux_stores::Step::Download { url, .. }
            | lapinux_stores::Step::EpicBuild { url, .. } => url.as_str(),
            lapinux_stores::Step::Unzip { file }
            | lapinux_stores::Step::RunInstaller { file }
            | lapinux_stores::Step::InnoExtract { file } => file.as_str(),
        };
        for byte in text.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    format!("{hash:016x}")
}

/// Nom de rayon d'une boutique, tel quel : « Epic Games », « GOG »,
/// « itch.io ». `safe_dirname` retire les points — utile contre un `..` venu
/// de la boutique, mais qui transformait itch.io en « itchio ».
fn shelf_name(label: &str) -> String {
    let cleaned: String = label
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | '\0') && !c.is_control())
        .collect();
    let cleaned = cleaned.trim().trim_matches('.');
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "Boutique".to_string()
    } else {
        cleaned.to_string()
    }
}

fn safe_dirname(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter_map(|c| {
            // Les points disparaissent : `..` remonterait hors de `common`.
            if c == '.' {
                None
            } else if c.is_alphanumeric() || " _-()[]'".contains(c) {
                Some(c)
            } else {
                Some('_')
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('_').to_string();
    if trimmed.is_empty() {
        "Jeu".into()
    } else {
        trimmed
    }
}

/// Poids réel d'un dossier, sans suivre les liens symboliques.
fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_symlink() => {}
            Ok(kind) if kind.is_dir() => total += dir_size(&path),
            Ok(_) => total += entry.metadata().map(|meta| meta.len()).unwrap_or(0),
            Err(_) => {}
        }
    }
    total
}

/// Premier exécutable du dossier, sous quelques niveaux : un jeu GOG met son
/// binaire dans `game/`, un jeu itch à la racine ou sous `bin/`.
fn find_executable(dir: &Path, name: &str) -> Option<PathBuf> {
    let target: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let mut candidates = Vec::new();
    collect_executables(dir, 0, &mut candidates);
    candidates.into_iter().min_by_key(|path| {
        let stem: String = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_lowercase())
            .collect();
        let score = if !target.is_empty() && stem.contains(&target) {
            0
        } else if stem.contains("launcher") || stem.contains("unins") {
            2
        } else {
            1
        };
        (score, path.components().count())
    })
}

fn collect_executables(dir: &Path, depth: u32, found: &mut Vec<PathBuf>) {
    if depth > 3 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            collect_executables(&path, depth + 1, found);
            continue;
        }
        if !kind.is_file() {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let executable = entry
                .metadata()
                .map(|meta| meta.permissions().mode() & 0o111 != 0)
                .unwrap_or(false);
            let suffix = path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("exe"))
                .unwrap_or(true);
            if executable && suffix {
                found.push(path);
            }
        }
        #[cfg(not(unix))]
        {
            let _ = path;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// L'empreinte suit les adresses du build : un nouveau build publié par la
    /// boutique la change, réinstaller le même jeu la laisse identique.
    #[test]
    fn a_build_fingerprint_follows_the_published_files() {
        let plan = |url: &str| lapinux_stores::InstallPlan {
            store: Store::Gog,
            store_id: "1207658691".into(),
            name: "The Witcher 3".into(),
            installdir: "The Witcher 3".into(),
            size: 10,
            installed: 0,
            platform: "windows".into(),
            build: String::new(),
            steps: vec![lapinux_stores::Step::Download {
                url: url.into(),
                file: "a.bin".into(),
                size: 10,
            }],
        };
        let first = plan_fingerprint(&plan("https://cdn/b1/a.bin"));
        assert_eq!(first, plan_fingerprint(&plan("https://cdn/b1/a.bin")));
        assert_ne!(first, plan_fingerprint(&plan("https://cdn/b2/a.bin")));
        assert_eq!(first.len(), 16);
    }

    #[test]
    fn an_epic_build_fingerprint_ignores_the_cdn_token() {
        let plan = |url: &str| lapinux_stores::InstallPlan {
            store: Store::Epic,
            store_id: "d6407c9e6fd54cb492b8c6635480d792".into(),
            name: "A Short Hike".into(),
            installdir: "A Short Hike".into(),
            size: 10,
            installed: 20,
            platform: "windows".into(),
            build: "epic-build".into(),
            steps: vec![lapinux_stores::Step::EpicBuild {
                url: url.into(),
                size: 10,
            }],
        };
        assert_eq!(
            plan_fingerprint(&plan("https://cdn/a.manifest?token=1")),
            plan_fingerprint(&plan("https://cdn/b.manifest?token=2"))
        );
    }

    #[test]
    fn without_an_account_every_store_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let paths = lapinux_core::paths::LapinuxPaths::under(dir.path());
        let shared = std::sync::Arc::new(Shared::new(&paths).unwrap());
        let view: StoresView = shared.stores_view(false, &[]).unwrap().into_data().unwrap();
        // Les trois boutiques s'affichent même sans compte : c'est là qu'on
        // vient en connecter un.
        assert_eq!(view.accounts.len(), 3);
        assert!(view.accounts.iter().all(|account| !account.connected));
        // Epic et GOG s'autorisent dans le navigateur ; itch.io n'a pas de
        // flux OAuth, son lien mène à la page où créer la clé d'API.
        for store in [Store::Epic, Store::Gog, Store::Itch] {
            let account = view
                .accounts
                .iter()
                .find(|account| account.store == store)
                .unwrap();
            assert!(!account.login_url.is_empty(), "{}", store.label());
            assert!(!account.login_hint.is_empty(), "{}", store.label());
        }
        let itch = view
            .accounts
            .iter()
            .find(|account| account.store == Store::Itch)
            .unwrap();
        assert!(itch.login_url.contains("api-keys"));
        assert!(view
            .libraries
            .iter()
            .all(|library| library.entries.is_empty()));
    }

    #[test]
    fn cloud_paths_stay_under_the_save_folder() {
        // Un nom venu du cloud ne peut pas remonter hors du dossier.
        assert!(safe_relative("../secret").is_none());
        assert!(safe_relative("/etc/passwd").is_none());
        assert_eq!(
            safe_relative("slot1/save.sav"),
            Some(std::path::PathBuf::from("slot1/save.sav"))
        );
        assert_eq!(
            safe_relative("./save.sav"),
            Some(std::path::PathBuf::from("save.sav"))
        );
    }

    #[test]
    fn local_saves_are_listed_with_slashes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("slot1")).unwrap();
        std::fs::write(dir.path().join("profil.sav"), b"a").unwrap();
        std::fs::write(dir.path().join("slot1").join("save.sav"), b"b").unwrap();
        let mut found = Vec::new();
        collect_saves(dir.path(), dir.path(), &mut found);
        let mut names: Vec<String> = found.into_iter().map(|(name, _)| name).collect();
        names.sort();
        // Le séparateur est toujours `/`, même sur un système qui en préfère un
        // autre : c'est la forme que le cloud attend.
        assert_eq!(names, vec!["profil.sav".to_string(), "slot1/save.sav".to_string()]);
    }

    #[test]
    fn a_platform_is_preferred_in_order() {
        // Linux d'abord, même quand Windows est annoncé en premier.
        assert_eq!(
            preferred(&["windows".into(), "linux".into()]),
            Some("linux".to_string())
        );
        // Sans Linux, Windows ; sans rien, aucun choix imposé.
        assert_eq!(preferred(&["macos".into()]), Some("macos".to_string()));
        assert_eq!(preferred(&[]), None);
    }

    #[test]
    fn a_library_keeps_one_line_per_game() {
        let entry = |id: &str, name: &str, installed: bool| core_stores::StoreGame {
            store: Store::Epic,
            store_id: id.into(),
            appid: core_stores::store_appid(Store::Epic, id),
            name: name.into(),
            installed,
            ..Default::default()
        };
        let mut entries = vec![
            entry("bundle", "Shadow Tactics Blades of the Shogun", false),
            entry("bundle", "Shadow Tactics Blades of the Shogun", false),
            entry("game", "Shadow Tactics: Blades of the Shogun", true),
            entry("v", "GTA V", false),
            entry("enhanced", "GTA V Enhanced", false),
        ];
        dedupe_entries(&mut entries);
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Shadow Tactics: Blades of the Shogun", "GTA V", "GTA V Enhanced"]
        );
        // La copie installée gagne : c'est son identifiant que la fiche lance.
        assert_eq!(entries[0].store_id, "game");
        assert!(entries[0].installed);
    }

    #[test]
    fn installdirs_cannot_escape_the_library() {
        // Un chemin venu de la boutique ne doit pas sortir de `common`.
        assert_eq!(safe_dirname("../../etc"), "etc");
        assert_eq!(safe_dirname("Disco Elysium"), "Disco Elysium");
        assert_eq!(safe_dirname("  "), "Jeu");
        assert!(!safe_dirname("../..").contains('/'));
    }

    #[test]
    fn the_weight_of_a_folder_adds_up() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), vec![0u8; 10]).unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("b"), vec![0u8; 5]).unwrap();
        assert_eq!(dir_size(dir.path()), 15);
        // Un dossier absent ne pèse rien plutôt que de faire échouer le scan.
        assert_eq!(dir_size(&dir.path().join("absent")), 0);
    }
}
