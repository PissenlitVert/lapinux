//! Sauvegardes locales versionnées : une copie datée du dossier de
//! sauvegarde d'un jeu, rangée hors de son préfixe et de son dossier
//! d'installation. C'est le filet des jeux sans cloud — et une version de plus
//! pour les autres.

use lapinux_core::config::Config;
use lapinux_core::model::{Game, SaveEvent, SaveSnapshot};
use lapinux_core::paths::LapinuxPaths;
use lapinux_core::store::Store;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Trois emplacements par jeu, comme une console : les copies au-delà sont
/// retirées, ce qui borne l'espace disque.
pub const SLOTS: u8 = 3;

/// Racine des copies : celle des réglages, sinon `Sauvegardes`.
pub fn root(paths: &LapinuxPaths, config: &Config) -> PathBuf {
    let chosen = config.general.saves_root.trim();
    if chosen.is_empty() {
        return paths.saves_root();
    }
    let chosen = lapinux_core::paths::expand_tilde(chosen);
    // Ancien emplacement par défaut : les copies ont suivi au premier
    // démarrage, sans changer le réglage enregistré.
    if chosen == paths.data_dir.join("saves") && !chosen.is_dir() {
        return paths.saves_root();
    }
    chosen
}

/// Nom du dossier d'un jeu : son titre lisible, l'identifiant entre
/// parenthèses pour rester unique. Un titre vide retombe sur l'identifiant.
pub fn folder_label(game: &Game) -> String {
    let mut name = String::new();
    for ch in game.name.trim().chars() {
        // Un séparateur de chemin ou un caractère de contrôle casserait le
        // dossier ; le reste du titre, accents compris, est conservé.
        name.push(if ch.is_control() || ch == '/' {
            '-'
        } else {
            ch
        });
        // La limite est en octets : un titre en emoji ne doit pas dépasser la
        // longueur d'un nom de fichier une fois l'identifiant ajouté.
        if name.len() >= 120 {
            break;
        }
    }
    let name = name.trim().trim_end_matches('.').trim();
    if name.is_empty() {
        game.appid.to_string()
    } else {
        format!("{name} ({})", game.appid)
    }
}

/// Nom d'une copie : son emplacement puis sa date. Sans lui, un dossier de
/// sauvegarde n'était qu'une suite de secondes Unix, illisible dans le
/// gestionnaire de fichiers.
pub fn copy_label(slot: u8, created: i64) -> String {
    // « 31/10/2026 à 02:50 » devient « 31-10-2026 02-50 » : les barres et les
    // deux-points sont interdits dans un nom de dossier sur certains systèmes.
    let stamp = lapinux_core::util::format_datetime(created)
        .replace(" à ", " ")
        .replace('/', "-")
        .replace(':', "-");
    format!("{slot} ({stamp})")
}

/// Dossier d'un jeu dans la racine des copies. Les copies d'avant le nom
/// lisible vivent sous `<appid>` ; elles suivent le jeu une fois, et un titre
/// corrigé renomme le dossier au passage. Un renommage impossible laisse le
/// dossier existant en place : aucune copie ne doit disparaître.
fn game_root(game: &Game, paths: &LapinuxPaths, config: &Config) -> PathBuf {
    let root = root(paths, config);
    let wanted = root.join(folder_label(game));
    if wanted.is_dir() {
        return wanted;
    }
    let suffix = format!(" ({})", game.appid);
    let legacy = game.appid.to_string();
    for entry in std::fs::read_dir(&root).into_iter().flatten().flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == legacy || name.ends_with(&suffix) {
            if std::fs::rename(entry.path(), &wanted).is_ok() {
                return wanted;
            }
            return entry.path();
        }
    }
    wanted
}

/// Ce que la fiche d'un jeu écrit pour décrire une copie : le manifeste vit à
/// côté du dossier de fichiers, pour qu'une restauration ne recopie jamais le
/// manifeste dans les sauvegardes du jeu.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Manifest {
    id: String,
    /// Emplacement occupé, de 1 à 3. Zéro pour un manifeste antérieur au
    /// découpage en emplacements.
    #[serde(default)]
    slot: u8,
    created: i64,
    files: usize,
    bytes: u64,
    source: String,
    /// Nombre de fichiers, poids et date la plus récente : deux copies
    /// identiques ne s'empilent pas.
    fingerprint: String,
}

impl Manifest {
    /// Décrit la copie ; `dir` est le dossier qui la contient, pour que la
    /// fiche puisse l'ouvrir dans le gestionnaire de fichiers.
    fn snapshot(&self, dir: &Path) -> SaveSnapshot {
        SaveSnapshot {
            id: self.id.clone(),
            slot: self.slot,
            created: self.created,
            files: self.files,
            bytes: self.bytes,
            source: self.source.clone(),
            path: dir.join(&self.id).display().to_string(),
        }
    }
}

/// Dossier de sauvegarde d'un jeu. Dans l'ordre : le dossier choisi dans les
/// réglages du jeu, celui de sa boutique (`sauvegardes/` à côté du jeu), un
/// dossier évident dans son installation, puis les emplacements Windows de son
/// préfixe — ce dernier reste celui d'un seul jeu, chaque préfixe étant dédié.
pub fn source(game: &Game, paths: &LapinuxPaths) -> Option<PathBuf> {
    if let Some(dir) = game
        .settings
        .save_dir
        .as_deref()
        .map(str::trim)
        .filter(|dir| !dir.is_empty())
    {
        let path = lapinux_core::paths::expand_tilde(dir);
        let path = if path.is_absolute() {
            path
        } else {
            game.install_dir().join(path)
        };
        if path.is_dir() {
            return Some(path);
        }
    }
    let install = game.install_dir();
    if !install.as_os_str().is_empty() {
        for name in ["sauvegardes", "saves", "save", "SaveData", "Saved Games"] {
            let dir = install.join(name);
            if dir.is_dir() {
                return Some(dir);
            }
        }
    }
    // Le préfixe du jeu, puis l'ancien rangement sous les données : une
    // lecture ne déplace rien, mais elle doit trouver les deux.
    let mut prefixes = vec![lapinux_proton::prefix_path(game, &paths.data_dir)];
    // Un préfixe déplacé à la main ou noté dans une ancienne entrée reste
    // lisible, sans déclencher son déménagement.
    if let Some(existing) = &game.prefix {
        if !prefixes.iter().any(|prefix| prefix == existing) {
            prefixes.push(existing.clone());
        }
    }
    let legacy = paths.data_dir.join("prefixes").join(game.appid.to_string());
    if prefixes.iter().all(|prefix| prefix != &legacy) {
        prefixes.push(legacy);
    }
    for prefix in prefixes {
        for base in ["pfx/drive_c/users/steamuser", "drive_c/users/steamuser"] {
            let user = prefix.join(base);
            if !user.is_dir() {
                continue;
            }
            // Un jeu Windows pose ses sauvegardes dans un dossier à son nom :
            // `AppData/Local/MotorSlice/Saved/SaveGames` pour Unreal,
            // `AppData/LocalLow/<éditeur>/A Short Hike` pour Unity. Ce dossier
            // passe avant « Saved Games », que Proton crée vide dans chaque
            // préfixe : sinon aucun jeu n'aurait de copie automatique.
            if let Some(dir) = named_folder(&user, &game.name) {
                // Unreal range ses sauvegardes dans `Saved/SaveGames`. Le
                // dossier du jeu, lui, porte aussi les caches d'assets et les
                // maps Workshop : les copier ferait des sauvegardes de
                // plusieurs centaines de Mo pour quelques kilo-octets utiles.
                let unreal = dir.join("Saved/SaveGames");
                return Some(if unreal.is_dir() { unreal } else { dir });
            }
            for rel in ["Saved Games", "Documents/My Games"] {
                let dir = user.join(rel);
                if dir.is_dir() {
                    return Some(dir);
                }
            }
        }
    }
    None
}

/// Dossiers où les jeux Windows écrivent, sous le profil du préfixe.
const SAVE_ROOTS: [&str; 5] = [
    "AppData/Local",
    "AppData/LocalLow",
    "AppData/Roaming",
    "Documents",
    "Saved Games",
];

/// Compare un nom de dossier au nom du jeu, sans casse, espaces ni
/// ponctuation : « MOTORSLICE » et « MotorSlice » sont le même jeu, et un
/// dossier plus long qui commence par le nom (« A Short Hike Demo ») reste
/// plausible.
fn folder_name_matches(path: &Path, key: &str) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let folder = normalize(name);
    if folder.is_empty() {
        return false;
    }
    folder == key
        || (folder.len() >= 5 && key.starts_with(&folder))
        || (key.len() >= 5 && folder.starts_with(key))
}

fn normalize(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

/// Cherche, dans les dossiers de sauvegarde du profil, celui qui porte le nom
/// du jeu. Un niveau pour`AppData/Local/MotorSlice`, deux pour la convention
/// Unity `AppData/LocalLow/<éditeur>/<jeu>`.
fn named_folder(user: &Path, name: &str) -> Option<PathBuf> {
    let key = normalize(name);
    if key.len() < 3 {
        return None;
    }
    // Le nom exact d'abord, puis les voisins : l'ordre du disque ne décide
    // pas de la copie, et deux correspondances restent triées.
    let mut matches: Vec<(usize, PathBuf)> = Vec::new();
    for rel in SAVE_ROOTS {
        let Ok(entries) = std::fs::read_dir(user.join(rel)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if folder_name_matches(&path, &key) {
                matches.push((0, path));
                continue;
            }
            let Ok(children) = std::fs::read_dir(&path) else {
                continue;
            };
            for child in children.flatten() {
                let child = child.path();
                if child.is_dir() && folder_name_matches(&child, &key) {
                    matches.push((1, child));
                }
            }
        }
    }
    matches.sort();
    matches.into_iter().next().map(|(_, path)| path)
}

/// Nombre de fichiers, poids total et date de modification la plus récente.
/// Le parcours ne suit pas les liens symboliques : un `pfx -> .` de préfixe
/// ferait sinon boucler la mesure.
fn measure(dir: &Path) -> io::Result<(usize, u64, i64)> {
    fn walk(dir: &Path, files: &mut usize, bytes: &mut u64, newest: &mut i64) -> io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                *files += 1;
                continue;
            }
            let meta = entry.metadata()?;
            if meta.is_dir() {
                walk(&entry.path(), files, bytes, newest)?;
            } else {
                *files += 1;
                *bytes += meta.len();
                if let Ok(modified) = meta.modified() {
                    if let Ok(since) = modified.duration_since(UNIX_EPOCH) {
                        *newest = (*newest).max(since.as_secs() as i64);
                    }
                }
            }
        }
        Ok(())
    }
    let mut files = 0;
    let mut bytes = 0;
    let mut newest = 0;
    walk(dir, &mut files, &mut bytes, &mut newest)?;
    Ok((files, bytes, newest))
}

/// Empreinte du dossier : chemins relatifs et contenu, sans les dates. Une
/// sauvegarde réécrite dans la même seconde avec la même taille est donc vue
/// comme différente, et deux copies identiques restent reconnues.
fn fingerprint(dir: &Path) -> io::Result<(usize, u64, u64)> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::io::Read;

    fn relative(path: &Path, base: &Path) -> String {
        path.strip_prefix(base)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    fn walk(
        dir: &Path,
        base: &Path,
        files: &mut usize,
        bytes: &mut u64,
        hasher: &mut DefaultHasher,
    ) -> io::Result<()> {
        // L'ordre du système de fichiers n'est pas garanti : sans tri, deux
        // lectures du même dossier pourraient rendre deux empreintes.
        let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let file_type = entry.file_type()?;
            relative(&path, base).hash(hasher);
            if file_type.is_dir() {
                walk(&path, base, files, bytes, hasher)?;
                continue;
            }
            *files += 1;
            if file_type.is_symlink() {
                if let Ok(target) = std::fs::read_link(&path) {
                    target.to_string_lossy().hash(hasher);
                }
                continue;
            }
            let mut file = std::fs::File::open(&path)?;
            let mut buffer = [0u8; 64 * 1024];
            loop {
                let read = file.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                *bytes += read as u64;
                hasher.write(&buffer[..read]);
            }
        }
        Ok(())
    }

    let mut files = 0;
    let mut bytes = 0;
    let mut hasher = DefaultHasher::new();
    walk(dir, dir, &mut files, &mut bytes, &mut hasher)?;
    Ok((files, bytes, hasher.finish()))
}

/// Manifestes du jeu, du plus récent au plus ancien.
fn manifests(dir: &Path) -> Vec<Manifest> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") || !path.is_file() {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(manifest) = serde_json::from_str::<Manifest>(&text) {
            out.push(manifest);
        }
    }
    out.sort_by(|left, right| {
        right
            .created
            .cmp(&left.created)
            .then(right.id.cmp(&left.id))
    });
    out
}

fn write_manifest(dir: &Path, manifest: &Manifest) -> io::Result<()> {
    let text = serde_json::to_string_pretty(manifest).map_err(io::Error::other)?;
    std::fs::write(dir.join(format!("{}.json", manifest.id)), text)
}

fn remove_copy(dir: &Path, manifest: &Manifest) {
    let _ = std::fs::remove_dir_all(dir.join(&manifest.id));
    let _ = std::fs::remove_file(dir.join(format!("{}.json", manifest.id)));
}

/// Range les copies dans les trois emplacements : celles qui en ont déjà un le
/// gardent ; les manifestes antérieurs reçoivent la première place libre, de la
/// plus récente à la plus ancienne ; tout ce qui dépasse est retiré. Cette
/// normalisation libère aussi l'espace des anciennes séries de dix copies.
fn settle(dir: &Path) -> Vec<Manifest> {
    let manifests = manifests(dir);
    if manifests.is_empty() {
        return Vec::new();
    }
    let mut slots: [Option<Manifest>; SLOTS as usize] = Default::default();
    // Les emplacements déjà attribués gardent leur copie (la plus récente en
    // cas de doublon).
    for manifest in &manifests {
        if (1..=SLOTS).contains(&manifest.slot) {
            let index = manifest.slot as usize - 1;
            if slots[index].is_none() {
                slots[index] = Some(manifest.clone());
            }
        }
    }
    let mut assigned: Vec<String> = slots
        .iter()
        .flatten()
        .map(|manifest| manifest.id.clone())
        .collect();
    // Les copies sans emplacement remplissent les places libres.
    for manifest in &manifests {
        if (1..=SLOTS).contains(&manifest.slot) {
            continue;
        }
        let Some(index) = slots.iter().position(|slot| slot.is_none()) else {
            break;
        };
        let mut copy = manifest.clone();
        copy.slot = index as u8 + 1;
        // Un manifeste illisible ne fait pas perdre la copie : elle reste
        // listée et sera réessayée à la prochaine lecture.
        let _ = write_manifest(dir, &copy);
        slots[index] = Some(copy);
        assigned.push(manifest.id.clone());
    }
    // Doublons d'emplacement et copies au-delà de trois : retirés.
    for manifest in &manifests {
        if !assigned.iter().any(|id| id == &manifest.id) {
            remove_copy(dir, manifest);
        }
    }
    // Les copies d'avant le nom lisible prennent le leur : emplacement et
    // date, une fois l'emplacement attribué. Le dossier part avant le
    // manifeste ; si celui-ci ne s'écrit pas, le dossier revient en place.
    for manifest in slots.iter_mut().flatten() {
        let wanted = copy_label(manifest.slot, manifest.created);
        if manifest.id == wanted {
            continue;
        }
        let from_dir = dir.join(&manifest.id);
        let from_manifest = dir.join(format!("{}.json", manifest.id));
        let to_dir = dir.join(&wanted);
        let to_manifest = dir.join(format!("{wanted}.json"));
        if !from_dir.is_dir() || to_dir.exists() || to_manifest.exists() {
            continue;
        }
        if std::fs::rename(&from_dir, &to_dir).is_err() {
            continue;
        }
        let mut renamed = manifest.clone();
        renamed.id = wanted;
        if write_manifest(dir, &renamed).is_ok() {
            let _ = std::fs::remove_file(&from_manifest);
            *manifest = renamed;
        } else {
            let _ = std::fs::rename(&to_dir, &from_dir);
        }
    }
    slots.into_iter().flatten().collect()
}

/// Normalise tous les jeux au démarrage : les anciennes séries de dix copies
/// libèrent tout de suite l'espace des trois emplacements gardés.
pub fn settle_all(paths: &LapinuxPaths, config: &Config) {
    let Ok(entries) = std::fs::read_dir(root(paths, config)) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.path().is_dir() {
            settle(&entry.path());
        }
    }
}

/// Copies d'un jeu, dans l'ordre des emplacements.
pub fn list(game: &Game, paths: &LapinuxPaths, config: &Config) -> Vec<SaveSnapshot> {
    let dir = game_root(game, paths, config);
    settle(&dir)
        .iter()
        .map(|manifest| manifest.snapshot(&dir))
        .collect()
}

/// Retire une copie des emplacements : son dossier et son manifeste. Rend la
/// copie supprimée pour que le journal garde ce qui a disparu.
pub fn remove(
    game: &Game,
    paths: &LapinuxPaths,
    config: &Config,
    id: &str,
) -> Result<SaveSnapshot, String> {
    // L'identifiant est un nom de dossier : on refuse seulement ce qui
    // pourrait sortir du dossier des sauvegardes.
    if id.is_empty() || id.contains('/') || id.contains('\\') || id.starts_with('.') {
        return Err("identifiant de copie inattendu".into());
    }
    let dir = game_root(game, paths, config);
    let manifest = settle(&dir)
        .into_iter()
        .find(|manifest| manifest.id == id)
        .ok_or_else(|| "cette copie n'existe plus".to_string())?;
    let snapshot = manifest.snapshot(&dir);
    let target = dir.join(&manifest.id);
    if target.exists() {
        std::fs::remove_dir_all(&target)
            .map_err(|error| format!("{} : {error}", target.display()))?;
    }
    match std::fs::remove_file(dir.join(format!("{}.json", manifest.id))) {
        Ok(()) => Ok(snapshot),
        Err(error) => Err(format!(
            "{} : {error}",
            dir.join(format!("{}.json", manifest.id)).display()
        )),
    }
}

/// Opérations gardées par jeu : le journal raconte les gestes récents, il ne
/// grossit pas sans fin.
const HISTORY_MAX: usize = 200;

fn history_key(appid: u32) -> String {
    format!("save-events-{appid}")
}

/// Journal d'un jeu, du plus récent au plus ancien.
pub fn events(store: &Store, appid: u32) -> Vec<SaveEvent> {
    let mut events = store
        .get_json::<Vec<SaveEvent>>(&history_key(appid))
        .ok()
        .flatten()
        .unwrap_or_default();
    events.reverse();
    events
}

/// Journal complet du jeu : les opérations enregistrées, plus une ligne
/// « copie locale » pour chaque copie antérieure au journal. Sans elle, une
/// copie visible dans les emplacements manquerait à l'historique.
pub fn history(store: &Store, appid: u32, copies: &[SaveSnapshot]) -> Vec<SaveEvent> {
    let mut events = events(store, appid);
    let known: std::collections::HashSet<String> = events
        .iter()
        .filter(|event| !event.snapshot.is_empty())
        .map(|event| event.snapshot.clone())
        .collect();
    for copy in copies {
        if known.contains(&copy.id) {
            continue;
        }
        events.push(SaveEvent {
            at: copy.created,
            kind: lapinux_core::model::SaveEventKind::Backup,
            snapshot: copy.id.clone(),
            files: copy.files,
            bytes: copy.bytes,
            detail: copy.source.clone(),
            imported: true,
            ..Default::default()
        });
    }
    events.sort_by(|left, right| right.at.cmp(&left.at));
    events
}

/// Ajoute une opération au journal en gardant les plus récentes. Un journal
/// illisible ne bloque jamais une copie ou une restauration : il repart d'une
/// ligne.
pub fn note(store: &Store, appid: u32, event: SaveEvent) {
    let key = history_key(appid);
    let mut events = store
        .get_json::<Vec<SaveEvent>>(&key)
        .ok()
        .flatten()
        .unwrap_or_default();
    events.push(event);
    if events.len() > HISTORY_MAX {
        let extra = events.len() - HISTORY_MAX;
        events.drain(..extra);
    }
    if let Err(error) = store.set_json(&key, &events) {
        tracing::warn!("journal des sauvegardes de {appid} : {error}");
    }
}

/// Poids total de toutes les copies.
pub fn total_bytes(paths: &LapinuxPaths, config: &Config) -> u64 {
    measure(&root(paths, config))
        .map(|(_, bytes, _)| bytes)
        .unwrap_or(0)
}

/// Bilan de toutes les copies : jeux concernés, nombre de copies, poids et
/// racine effective. Les réglages l'affichent tel quel.
pub fn overview(paths: &LapinuxPaths, config: &Config) -> serde_json::Value {
    let root = root(paths, config);
    let mut games = 0usize;
    let mut copies = 0usize;
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            // Le bilan normalise aussi les anciennes séries : les copies
            // au-delà de trois libèrent leur place ici.
            let count = settle(&entry.path()).len();
            if count > 0 {
                games += 1;
                copies += count;
            }
        }
    }
    serde_json::json!({
        "root": root.display().to_string(),
        "games": games,
        "copies": copies,
        "bytes": total_bytes(paths, config),
    })
}

/// Prend une copie dans la première place libre, ou remplace la plus
/// ancienne quand les trois emplacements sont pris.
pub fn backup(
    game: &Game,
    paths: &LapinuxPaths,
    config: &Config,
) -> Result<Option<SaveSnapshot>, String> {
    backup_to_slot(game, paths, config, None)
}

/// Prend une copie des sauvegardes dans un emplacement donné. Sans `slot`,
/// remplit la première place libre puis remplace la copie la plus ancienne.
/// Rend `None` quand il n'y a rien à copier, ou quand cet état est déjà gardé
/// dans l'emplacement visé : l'appelant peut alors dire « rien de neuf »
/// plutôt que d'empiler un doublon.
pub fn backup_to_slot(
    game: &Game,
    paths: &LapinuxPaths,
    config: &Config,
    slot: Option<u8>,
) -> Result<Option<SaveSnapshot>, String> {
    backup_choice(game, paths, config, slot, None)
}

/// Copie vérifiée exigée avant une opération destructive. Un dossier vide ou
/// indéterminé n'est pas une preuve qu'aucune sauvegarde n'existe ailleurs.
pub fn verified_backup(
    game: &Game,
    paths: &LapinuxPaths,
    config: &Config,
) -> Result<String, String> {
    if let Some(configured) = game
        .settings
        .save_dir
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
    {
        let configured = lapinux_core::paths::expand_tilde(configured);
        let configured = if configured.is_absolute() {
            configured
        } else {
            game.install_dir().join(configured)
        };
        if !configured.is_dir() {
            return Err(format!(
                "dossier de sauvegarde configuré introuvable : {}",
                configured.display()
            ));
        }
    }
    let source = source(game, paths).ok_or("dossier de sauvegarde indéterminé")?;
    ensure_recoverable_tree(&source)?;
    let expected = fingerprint(&source).map_err(|e| format!("{} : {e}", source.display()))?;
    if expected.0 == 0 {
        return Err(
            "le dossier détecté est vide : les sauvegardes ne peuvent pas être garanties".into(),
        );
    }
    let source_canonical = std::fs::canonicalize(&source).map_err(|e| e.to_string())?;
    let backup_root = root(paths, config);
    std::fs::create_dir_all(&backup_root).map_err(|e| format!("dossier des copies : {e}"))?;
    let root_canonical = std::fs::canonicalize(&backup_root).map_err(|e| e.to_string())?;
    if root_canonical.starts_with(&source_canonical)
        || std::fs::canonicalize(game.install_dir())
            .ok()
            .is_some_and(|dir| root_canonical.starts_with(dir))
        || game
            .prefix
            .as_ref()
            .and_then(|p| std::fs::canonicalize(p).ok())
            .is_some_and(|dir| root_canonical.starts_with(dir))
        || std::fs::canonicalize(lapinux_proton::prefix_path(game, &paths.data_dir))
            .ok()
            .is_some_and(|dir| root_canonical.starts_with(dir))
    {
        return Err("le dossier des copies se trouve dans les fichiers à supprimer".into());
    }
    backup(game, paths, config)?;
    let copies = list(game, paths, config);
    let verified = copies
        .iter()
        .find(|copy| fingerprint_dir_matches(Path::new(&copy.path), expected));
    if !fingerprint_dir_matches(&source, expected) {
        return Err("les sauvegardes ont changé pendant la copie".into());
    }
    let path = verified
        .map(|copy| copy.path.clone())
        .ok_or_else(|| "aucune copie de sécurité vérifiée".to_string())?;
    let copy_dir = std::fs::canonicalize(&path).map_err(|e| e.to_string())?;
    let mut removed_roots = vec![
        game.install_dir(),
        paths.data_dir.join("prefixes").join(game.appid.to_string()),
        game.library
            .join("steamapps/compatdata")
            .join(game.appid.to_string()),
        game.library.join("compatdata").join(game.appid.to_string()),
        game.library
            .join("CompatData")
            .join(lapinux_proton::prefix_name_for(game)),
    ];
    if let Some(prefix) = &game.prefix {
        removed_roots.push(prefix.clone());
    }
    let compat = game.library.join("CompatData");
    if let Ok(entries) = std::fs::read_dir(compat) {
        let suffix = format!(" ({})", game.appid);
        removed_roots.extend(
            entries
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().ends_with(&suffix))
                .map(|e| e.path()),
        );
    }
    if removed_roots
        .iter()
        .filter_map(|p| std::fs::canonicalize(p).ok())
        .any(|p| copy_dir.starts_with(p))
    {
        return Err("la copie se trouve dans un dossier qui serait supprimé".into());
    }
    Ok(path)
}

/// Même copie, en évitant un emplacement : la restauration range la copie de
/// sécurité ailleurs que la copie qu'elle s'apprête à relire, sinon elle
/// l'effacerait avant de s'en servir.
fn backup_choice(
    game: &Game,
    paths: &LapinuxPaths,
    config: &Config,
    slot: Option<u8>,
    avoid: Option<u8>,
) -> Result<Option<SaveSnapshot>, String> {
    let Some(source) = source(game, paths) else {
        return Ok(None);
    };
    let (files, bytes, digest) =
        fingerprint(&source).map_err(|error| format!("{} : {error}", source.display()))?;
    if files == 0 {
        return Ok(None);
    }
    let fingerprint = format!("{files}:{bytes}:{digest:016x}");
    let dir = game_root(game, paths, config);
    let existing = settle(&dir);
    let target_slot = match slot {
        Some(slot) if (1..=SLOTS).contains(&slot) => slot,
        Some(_) => return Err("emplacement de sauvegarde inconnu".into()),
        None => (1..=SLOTS)
            .find(|number| !existing.iter().any(|copy| copy.slot == *number))
            .or_else(|| {
                existing
                    .iter()
                    .filter(|copy| Some(copy.slot) != avoid)
                    .min_by_key(|copy| copy.created)
                    .map(|copy| copy.slot)
            })
            .or_else(|| {
                existing
                    .iter()
                    .min_by_key(|copy| copy.created)
                    .map(|copy| copy.slot)
            })
            .unwrap_or(1),
    };
    // Un état identique dans l'emplacement visé n'est pas recopié.
    let verified = |copy: &Manifest| {
        copy.fingerprint == fingerprint
            && fingerprint_dir_matches(&dir.join(&copy.id), (files, bytes, digest))
    };
    if existing
        .iter()
        .find(|copy| copy.slot == target_slot)
        .is_some_and(&verified)
    {
        return Ok(None);
    }
    // Un manifeste seul ne prouve pas que les fichiers sont encore intacts.
    if slot.is_none() && existing.iter().any(verified) {
        return Ok(None);
    }
    std::fs::create_dir_all(&dir).map_err(|error| format!("{} : {error}", dir.display()))?;
    let created = lapinux_core::util::now_unix();
    // Le dossier se nomme par son emplacement et sa date : lisible depuis le
    // gestionnaire de fichiers. Deux copies du même emplacement à la même
    // minute ne peuvent pas coexister, mais le suffixe garde le cas sûr.
    let base = copy_label(target_slot, created);
    let mut id = base.clone();
    let mut suffix = 2;
    while dir.join(&id).exists() || dir.join(format!("{id}.json")).exists() {
        id = format!("{base} ({suffix})");
        suffix += 1;
    }
    let target = dir.join(&id);
    if let Err(error) = lapinux_core::util::copy_tree(&source, &target) {
        let _ = std::fs::remove_dir_all(&target);
        return Err(format!("{} : {error}", source.display()));
    }
    if !fingerprint_dir_matches(&target, (files, bytes, digest))
        || !fingerprint_dir_matches(&source, (files, bytes, digest))
    {
        let _ = std::fs::remove_dir_all(&target);
        return Err(
            "la copie de sécurité ne correspond pas aux fichiers d'origine ou ils ont changé"
                .into(),
        );
    }
    let mut manifest = Manifest {
        id: id.clone(),
        slot: target_slot,
        created,
        files,
        bytes,
        source: source.display().to_string(),
        fingerprint,
    };
    if let Err(error) = write_manifest(&dir, &manifest) {
        let _ = std::fs::remove_dir_all(&target);
        return Err(format!("manifeste de sauvegarde : {error}"));
    }
    // L'ancienne copie de l'emplacement part seulement maintenant que la
    // nouvelle est complète.
    if let Some(previous) = existing.iter().find(|copy| copy.slot == target_slot) {
        remove_copy(&dir, previous);
    }
    // Deux copies du même emplacement dans la même minute : la seconde a reçu
    // un suffixe pour cohabiter. La place est libre maintenant, elle reprend le
    // nom simple — celui que la fiche affiche et ouvrira.
    if manifest.id != base
        && !dir.join(&base).exists()
        && !dir.join(format!("{base}.json")).exists()
    {
        let renamed_dir = dir.join(&base);
        if std::fs::rename(dir.join(&manifest.id), &renamed_dir).is_ok() {
            let mut renamed = manifest.clone();
            renamed.id = base;
            if write_manifest(&dir, &renamed).is_ok() {
                let _ = std::fs::remove_file(dir.join(format!("{}.json", manifest.id)));
                manifest = renamed;
            } else {
                let _ = std::fs::rename(&renamed_dir, dir.join(&manifest.id));
            }
        }
    }
    Ok(Some(manifest.snapshot(&dir)))
}

/// Les liens peuvent viser des fichiers effacés avec le préfixe : les copier
/// tels quels ne garantit pas qu'une sauvegarde sera récupérable.
fn ensure_recoverable_tree(dir: &Path) -> Result<(), String> {
    if std::fs::symlink_metadata(dir)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_symlink()
    {
        return Err(format!(
            "lien symbolique non sauvegardé : {}",
            dir.display()
        ));
    }
    for entry in std::fs::read_dir(dir).map_err(|e| format!("{} : {e}", dir.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_symlink() {
            return Err(format!(
                "lien symbolique non sauvegardé : {}",
                entry.path().display()
            ));
        }
        if kind.is_dir() {
            ensure_recoverable_tree(&entry.path())?;
        } else if !kind.is_file() {
            return Err(format!(
                "fichier spécial non sauvegardé : {}",
                entry.path().display()
            ));
        }
    }
    Ok(())
}

fn fingerprint_dir_matches(dir: &Path, expected: (usize, u64, u64)) -> bool {
    fingerprint(dir)
        .map(|actual| actual == expected)
        .unwrap_or(false)
}

/// Partie du chemin qui suit le profil Windows du préfixe
/// (`.../drive_c/users/steamuser/...`), quand la sauvegarde vit dans un
/// préfixe. Elle permet de retrouver le même dossier dans le préfixe actuel
/// du jeu, même si celui de la copie a été déplacé ou renommé.
fn profile_tail(path: &Path) -> Option<PathBuf> {
    let parts: Vec<&std::ffi::OsStr> = path.iter().collect();
    let win = parts.iter().position(|part| *part == "drive_c")?;
    if parts.get(win + 1).map(|part| *part) != Some("users".as_ref())
        || parts.get(win + 2).map(|part| *part) != Some("steamuser".as_ref())
    {
        return None;
    }
    let tail: PathBuf = parts[win + 3..].iter().collect();
    if tail.as_os_str().is_empty() {
        None
    } else {
        Some(tail)
    }
}

/// Où remettre une copie : le préfixe que le jeu utilise aujourd'hui. Un jeu
/// Windows relit ses sauvegardes dans son préfixe courant ; si celui de la
/// copie a changé (jeu réinstallé, identifiant différent), écrire à l'ancien
/// endroit ne mettrait rien dans le jeu. Hors préfixe, le dossier noté reste
/// la référence tant qu'il existe.
fn restore_target(game: &Game, paths: &LapinuxPaths, recorded: &Path) -> PathBuf {
    if let Some(tail) = profile_tail(recorded) {
        let mut prefix = lapinux_proton::prefix_path(game, &paths.data_dir);
        // Une ancienne entrée peut encore pointer un préfixe vivant que le
        // nouveau chemin ne connaît pas : écrire dedans est plus sûr que de
        // créer un préfixe vide à côté.
        if !prefix.is_dir() {
            if let Some(existing) = game.prefix.as_ref().filter(|prefix| prefix.is_dir()) {
                prefix = existing.clone();
            }
        }
        return prefix.join("pfx/drive_c/users/steamuser").join(tail);
    }
    if recorded.is_dir() {
        return recorded.to_path_buf();
    }
    source(game, paths).unwrap_or_else(|| recorded.to_path_buf())
}

/// Remet une copie à la place des sauvegardes du jeu. L'état courant est
/// d'abord copié : une restauration ne remplace jamais sans garder une trace.
/// Rend le nombre de fichiers remis.
pub fn restore(
    game: &Game,
    paths: &LapinuxPaths,
    config: &Config,
    id: &str,
) -> Result<usize, String> {
    // L'identifiant est un nom de dossier : on refuse seulement ce qui
    // pourrait sortir du dossier des sauvegardes.
    if id.is_empty() || id.contains('/') || id.contains('\\') || id.starts_with('.') {
        return Err("identifiant de copie inattendu".into());
    }
    let dir = game_root(game, paths, config);
    let snapshot = settle(&dir)
        .into_iter()
        .find(|manifest| manifest.id == id)
        .ok_or_else(|| "cette copie n'existe plus".to_string())?;
    let from = dir.join(&snapshot.id);
    if !from.is_dir() {
        return Err("les fichiers de cette copie sont absents".into());
    }
    let destination = restore_target(game, paths, Path::new(&snapshot.source));
    ensure_recoverable_tree(&from)?;
    if destination.exists() {
        ensure_recoverable_tree(&destination)?;
    }
    let selected = fingerprint(&from).map_err(|error| format!("copie illisible : {error}"))?;
    if format!("{}:{}:{:016x}", selected.0, selected.1, selected.2) != snapshot.fingerprint {
        return Err("la copie à restaurer est incomplète ou corrompue".into());
    }
    let current_state = if destination.exists() {
        Some(
            fingerprint(&destination)
                .map_err(|error| format!("sauvegarde actuelle illisible : {error}"))?,
        )
    } else {
        None
    };
    // Sauvegarder exactement la destination, pas un dossier deviné différent.
    if destination.exists() {
        if !destination.is_dir() {
            return Err("la destination des sauvegardes n'est pas un dossier".into());
        }
        let mut current = game.clone();
        current.settings.save_dir = Some(destination.to_string_lossy().into_owned());
        backup_choice(&current, paths, config, None, Some(snapshot.slot))
            .map_err(|error| format!("copie de sécurité impossible : {error}"))?;
    }
    if !from.is_dir() {
        return Err("les fichiers de cette copie ont disparu pendant la copie de sécurité".into());
    }
    let parent = destination
        .parent()
        .ok_or_else(|| format!("{} : dossier parent introuvable", destination.display()))?;
    std::fs::create_dir_all(parent).map_err(|error| format!("{} : {error}", parent.display()))?;
    // La copie est préparée à côté, puis le dossier du jeu est remplacé d'un
    // bloc : charger une sauvegarde doit donner exactement l'état choisi, pas
    // un mélange avec les fichiers de la partie en cours.
    let staging = parent.join(format!(".lapinux-saves-{}-new", snapshot.id));
    let previous = parent.join(format!(".lapinux-saves-{}-old", snapshot.id));
    if previous.exists() || staging.exists() {
        return Err(format!(
            "restauration précédente inachevée : vérifier {} et {}",
            previous.display(),
            staging.display()
        ));
    }
    lapinux_core::util::copy_tree(&from, &staging)
        .map_err(|error| format!("{} : {error}", staging.display()))?;
    let expected = fingerprint(&from).map_err(|error| format!("copie illisible : {error}"))?;
    if format!("{}:{}:{:016x}", expected.0, expected.1, expected.2) != snapshot.fingerprint
        || !fingerprint_dir_matches(&staging, expected)
    {
        let _ = std::fs::remove_dir_all(&staging);
        return Err("la copie à restaurer est incomplète ou corrompue".into());
    }
    let moved = destination.exists();
    if moved != current_state.is_some()
        || current_state.is_some_and(|state| !fingerprint_dir_matches(&destination, state))
    {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(
            "les sauvegardes ont changé pendant la préparation ; restauration annulée".into(),
        );
    }
    if moved {
        std::fs::rename(&destination, &previous)
            .map_err(|error| format!("{} : {error}", destination.display()))?;
    }
    if let Err(error) = std::fs::rename(&staging, &destination) {
        // Le dossier d'origine reprend sa place : un échec ne laisse pas le
        // jeu sans sauvegardes.
        if moved {
            let _ = std::fs::rename(&previous, &destination);
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!("{} : {error}", destination.display()));
    }
    let _ = std::fs::remove_dir_all(&previous);
    let (files, _, _) =
        measure(&destination).map_err(|error| format!("{} : {error}", destination.display()))?;
    Ok(files)
}

/// Recopie toutes les copies d'un jeu vers un dossier choisi. Les copies
/// locales restent en place : l'export est une seconde copie, pas un
/// déménagement.
pub fn export(
    game: &Game,
    paths: &LapinuxPaths,
    config: &Config,
    dest: &str,
) -> Result<PathBuf, String> {
    let dest = lapinux_core::paths::expand_tilde(dest.trim());
    if dest.as_os_str().is_empty() {
        return Err("dossier d'export manquant".into());
    }
    let from = game_root(game, paths, config);
    if !from.is_dir() {
        return Err("aucune copie locale pour ce jeu".into());
    }
    let to = dest.join(format!("lapinux-saves-{}", game.appid));
    std::fs::create_dir_all(&to).map_err(|error| format!("{} : {error}", to.display()))?;
    for entry in
        std::fs::read_dir(&from).map_err(|error| format!("{} : {error}", from.display()))?
    {
        let entry = entry.map_err(|error| format!("{} : {error}", from.display()))?;
        let target = to.join(entry.file_name());
        if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            lapinux_core::util::copy_tree(&entry.path(), &target)
                .map_err(|error| format!("{} : {error}", target.display()))?;
        } else {
            std::fs::copy(entry.path(), &target)
                .map_err(|error| format!("{} : {error}", target.display()))?;
        }
    }
    Ok(to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lapinux_core::config::Config;
    use lapinux_core::model::{Game, GameOrigin, GameSettings};
    use lapinux_core::stores::Store;

    struct Fixture {
        dir: tempfile::TempDir,
        paths: LapinuxPaths,
        game: Game,
        config: Config,
        saves: PathBuf,
    }

    impl Fixture {
        /// Un jeu de boutique installé dans une bibliothèque temporaire, avec
        /// son dossier `sauvegardes/` à côté de l'exécutable.
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let paths = LapinuxPaths::under(&dir.path().join("data"));
            paths.ensure().unwrap();
            let library = dir.path().join("LapinuxLibrary");
            let install = library.join("Epic Games/Test");
            let saves = install.join("sauvegardes");
            std::fs::create_dir_all(saves.join("slot")).unwrap();
            std::fs::write(saves.join("slot/niveau1.sav"), b"un").unwrap();
            std::fs::write(saves.join("slot/niveau2.sav"), b"deux").unwrap();
            let game = Game {
                appid: 42,
                name: "Test".into(),
                installdir: "Epic Games/Test".into(),
                library,
                origin: GameOrigin::Store,
                store: Store::Epic,
                settings: GameSettings::default(),
                ..Default::default()
            };
            let config = Config::default();
            Self {
                dir,
                paths,
                game,
                config,
                saves,
            }
        }
    }

    /// Un jeu qu'on installe ailleurs que sous son nom : pas de
    /// `sauvegardes/` à côté de l'exécutable, tout est dans le préfixe.
    fn game_without_shelf(fixture: &Fixture, name: &str) -> Game {
        Game {
            name: name.into(),
            installdir: "Epic Games/Autre".into(),
            ..fixture.game.clone()
        }
    }

    #[test]
    fn a_game_folder_carries_its_name_and_id() {
        let fixture = Fixture::new();
        assert_eq!(folder_label(&fixture.game), "Test (42)");
        // Un titre qui contient un séparateur ou un saut de ligne reste un
        // seul dossier, lisible.
        let mut game = fixture.game.clone();
        game.name = "A/B\nC".into();
        assert_eq!(folder_label(&game), "A-B-C (42)");
        // Un jeu sans nom garde son identifiant : le dossier existe toujours.
        game.name = "   ".into();
        assert_eq!(folder_label(&game), "42");
        let _ = fixture.dir;
    }

    #[test]
    fn an_old_id_folder_becomes_readable_without_losing_copies() {
        let fixture = Fixture::new();
        let legacy = root(&fixture.paths, &fixture.config).join("42");
        let copy = legacy.join("1700000000");
        std::fs::create_dir_all(&copy).unwrap();
        std::fs::write(copy.join("slot.sav"), b"progression").unwrap();
        std::fs::write(
            legacy.join("1700000000.json"),
            serde_json::to_string(&serde_json::json!({
                "id": "1700000000",
                "slot": 1,
                "created": 1_700_000_000,
                "files": 1,
                "bytes": 11,
                "source": "/tmp/jeu",
                "fingerprint": "1:11:1"
            }))
            .unwrap(),
        )
        .unwrap();
        let copies = list(&fixture.game, &fixture.paths, &fixture.config);
        assert_eq!(copies.len(), 1);
        assert_eq!(copies[0].files, 1);
        let named = root(&fixture.paths, &fixture.config).join("Test (42)");
        assert!(named.is_dir(), "le dossier a suivi le nom du jeu");
        assert!(!legacy.exists(), "l'ancien dossier ne reste pas en double");
        let _ = fixture.dir;
    }

    #[test]
    fn a_renamed_game_keeps_its_copies() {
        let fixture = Fixture::new();
        backup(&fixture.game, &fixture.paths, &fixture.config)
            .unwrap()
            .unwrap();
        let mut renamed = fixture.game.clone();
        renamed.name = "Nouveau titre".into();
        let copies = list(&renamed, &fixture.paths, &fixture.config);
        assert_eq!(copies.len(), 1);
        assert!(root(&fixture.paths, &fixture.config)
            .join("Nouveau titre (42)")
            .is_dir());
        let _ = fixture.dir;
    }

    #[test]
    fn a_windows_game_finds_the_unreal_save_folder() {
        let fixture = Fixture::new();
        let prefix = lapinux_proton::prefix_path(&fixture.game, &fixture.paths.data_dir);
        let root = prefix.join("pfx/drive_c/users/steamuser/AppData/Local/MotorSlice");
        let saves = root.join("Saved/SaveGames");
        std::fs::create_dir_all(&saves).unwrap();
        std::fs::write(saves.join("GameSave.sav"), b"progression").unwrap();
        let game = game_without_shelf(&fixture, "MOTORSLICE");
        // Unreal range ses sauvegardes dans `Saved/SaveGames` : le dossier du
        // jeu, lui, peut contenir des caches et des maps de plusieurs
        // centaines de Mo, que la copie ne doit pas embarquer.
        assert_eq!(source(&game, &fixture.paths), Some(saves));
        let _ = fixture.dir;
    }

    #[test]
    fn a_named_folder_without_unreal_savegames_is_kept() {
        let fixture = Fixture::new();
        let prefix = lapinux_proton::prefix_path(&fixture.game, &fixture.paths.data_dir);
        let root = prefix.join("pfx/drive_c/users/steamuser/AppData/Local/Test");
        let saved = root.join("Saved");
        std::fs::create_dir_all(&saved).unwrap();
        std::fs::write(saved.join("partie.sav"), b"x").unwrap();
        let game = game_without_shelf(&fixture, "Test");
        // Tous les jeux Unreal n'ont pas `Saved/SaveGames` : sans lui, le
        // dossier nommé reste la référence, comme avant.
        assert_eq!(source(&game, &fixture.paths), Some(root));
        let _ = fixture.dir;
    }

    #[test]
    fn a_unity_game_finds_its_folder_under_the_publisher() {
        let fixture = Fixture::new();
        let prefix = lapinux_proton::prefix_path(&fixture.game, &fixture.paths.data_dir);
        let saves =
            prefix.join("pfx/drive_c/users/steamuser/AppData/LocalLow/adamgryu/A Short Hike");
        std::fs::create_dir_all(&saves).unwrap();
        std::fs::write(saves.join("save.dat"), b"ile").unwrap();
        let game = game_without_shelf(&fixture, "A Short Hike");
        assert_eq!(source(&game, &fixture.paths), Some(saves));
        let _ = fixture.dir;
    }

    #[test]
    fn a_folder_of_another_game_is_not_taken_for_a_save() {
        let fixture = Fixture::new();
        let prefix = lapinux_proton::prefix_path(&fixture.game, &fixture.paths.data_dir);
        let other =
            prefix.join("pfx/drive_c/users/steamuser/AppData/Local/OtherGame/Saved/SaveGames");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("slot.sav"), b"x").unwrap();
        let game = game_without_shelf(&fixture, "MOTORSLICE");
        assert_eq!(source(&game, &fixture.paths), None);
        let _ = fixture.dir;
    }

    #[test]
    fn the_legacy_prefix_under_the_data_folder_is_still_read() {
        let fixture = Fixture::new();
        let root = fixture
            .paths
            .data_dir
            .join("prefixes/42/pfx/drive_c/users/steamuser/AppData/Local/MotorSlice");
        let saves = root.join("Saved/SaveGames");
        std::fs::create_dir_all(&saves).unwrap();
        std::fs::write(saves.join("GameSave.sav"), b"vieux").unwrap();
        let game = game_without_shelf(&fixture, "MotorSlice");
        assert_eq!(source(&game, &fixture.paths), Some(saves));
        let _ = fixture.dir;
    }

    #[test]
    fn a_backup_copies_the_save_folder_and_lists_it() {
        let fixture = Fixture::new();
        let snapshot = backup(&fixture.game, &fixture.paths, &fixture.config)
            .unwrap()
            .expect("une copie est prise");
        assert_eq!(snapshot.files, 2);
        assert!(snapshot.bytes > 0);
        let stored = game_root(&fixture.game, &fixture.paths, &fixture.config).join(&snapshot.id);
        assert!(stored.join("slot/niveau1.sav").is_file());
        assert!(!stored.join("manifeste.json").exists());
        let manifest = game_root(&fixture.game, &fixture.paths, &fixture.config)
            .join(format!("{}.json", snapshot.id));
        assert!(manifest.is_file());
        let listed = list(&fixture.game, &fixture.paths, &fixture.config);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, snapshot.id);
        assert_eq!(listed[0].source, fixture.saves.display().to_string());
        assert!(
            listed[0].path.ends_with(&snapshot.id),
            "la fiche doit pouvoir ouvrir le dossier de la copie : {}",
            listed[0].path
        );
        let _ = fixture.dir;
    }

    #[test]
    fn an_unchanged_save_folder_is_not_copied_again() {
        let fixture = Fixture::new();
        assert!(backup(&fixture.game, &fixture.paths, &fixture.config)
            .unwrap()
            .is_some());
        assert!(backup(&fixture.game, &fixture.paths, &fixture.config)
            .unwrap()
            .is_none());
        std::fs::write(fixture.saves.join("slot/niveau1.sav"), b"autre").unwrap();
        assert!(backup(&fixture.game, &fixture.paths, &fixture.config)
            .unwrap()
            .is_some());
        assert_eq!(
            list(&fixture.game, &fixture.paths, &fixture.config).len(),
            2
        );
        let _ = fixture.dir;
    }

    #[test]
    fn only_three_slots_are_kept() {
        let fixture = Fixture::new();
        for step in 0..5 {
            std::fs::write(
                fixture.saves.join("slot/niveau1.sav"),
                format!("version {step}"),
            )
            .unwrap();
            backup(&fixture.game, &fixture.paths, &fixture.config).unwrap();
        }
        let listed = list(&fixture.game, &fixture.paths, &fixture.config);
        assert_eq!(listed.len(), 3, "trois emplacements, pas dix");
        let mut slots: Vec<u8> = listed.iter().map(|copy| copy.slot).collect();
        slots.sort();
        assert_eq!(slots, vec![1, 2, 3]);
    }

    #[test]
    fn saving_into_a_slot_replaces_only_that_copy() {
        let fixture = Fixture::new();
        let first = backup_to_slot(&fixture.game, &fixture.paths, &fixture.config, Some(2))
            .unwrap()
            .unwrap();
        assert_eq!(first.slot, 2);
        std::fs::write(fixture.saves.join("slot/niveau1.sav"), b"encore").unwrap();
        let second = backup_to_slot(&fixture.game, &fixture.paths, &fixture.config, Some(2))
            .unwrap()
            .unwrap();
        assert_eq!(second.slot, 2);
        let listed = list(&fixture.game, &fixture.paths, &fixture.config);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, second.id);
        // Même emplacement, même minute : la nouvelle copie reprend le nom de
        // celle qu'elle remplace, et ce sont bien ses fichiers qui restent.
        let stored = game_root(&fixture.game, &fixture.paths, &fixture.config).join(&second.id);
        assert_eq!(
            std::fs::read(stored.join("slot/niveau1.sav")).unwrap(),
            b"encore",
            "l'ancienne copie a été remplacée"
        );
        let _ = first;
        // La place 1 reste libre : une copie générique la remplit.
        std::fs::write(fixture.saves.join("slot/niveau1.sav"), b"suite").unwrap();
        let third = backup(&fixture.game, &fixture.paths, &fixture.config)
            .unwrap()
            .unwrap();
        assert_eq!(third.slot, 1, "la première place libre est utilisée");
        let _ = fixture.dir;
    }

    #[test]
    fn older_copies_are_folded_into_three_slots() {
        let fixture = Fixture::new();
        let dir = game_root(&fixture.game, &fixture.paths, &fixture.config);
        // Cinq copies de l'ancien format, sans numéro d'emplacement.
        for created in [500_i64, 400, 300, 200, 100] {
            let id = created.to_string();
            let copy = dir.join(&id);
            std::fs::create_dir_all(&copy).unwrap();
            std::fs::write(copy.join("save.dat"), b"x").unwrap();
            let manifest = serde_json::json!({
                "id": id,
                "created": created,
                "files": 1,
                "bytes": 1,
                "source": "/tmp/ancien",
                "fingerprint": format!("1:1:{created}")
            });
            std::fs::write(
                dir.join(format!("{id}.json")),
                serde_json::to_string(&manifest).unwrap(),
            )
            .unwrap();
        }
        let listed = list(&fixture.game, &fixture.paths, &fixture.config);
        assert_eq!(listed.len(), 3);
        // Les trois plus récentes, de la plus récente à la plus ancienne.
        assert_eq!(listed[0].created, 500);
        assert_eq!(listed[0].slot, 1);
        assert_eq!(listed[1].slot, 2);
        assert_eq!(listed[2].slot, 3);
        assert!(
            !dir.join("200").exists(),
            "les copies en trop sont retirées"
        );
        assert!(!dir.join("100").exists());
        // L'emplacement est écrit une fois pour toutes, dans le manifeste de
        // la copie renommée « emplacement + date ».
        let manifest = dir.join(format!("{}.json", listed[0].id));
        let text = std::fs::read_to_string(&manifest).unwrap();
        assert!(text.contains("\"slot\": 1"), "manifeste migré : {text}");
        assert!(
            listed[0].id.starts_with("1 ("),
            "le dossier porte son emplacement et sa date : {}",
            listed[0].id
        );
    }

    #[test]
    fn restoration_stops_when_the_safety_copy_cannot_be_written() {
        let fixture = Fixture::new();
        let first = backup(&fixture.game, &fixture.paths, &fixture.config)
            .unwrap()
            .unwrap();
        std::fs::write(fixture.saves.join("slot/niveau1.sav"), b"current").unwrap();
        // Une socket Unix ne peut pas être lue comme un fichier de sauvegarde.
        // Passer par le descripteur du dossier évite la limite de longueur
        // sun_path lorsque le répertoire temporaire est profondément imbriqué.
        use std::os::fd::AsRawFd;
        let dir = std::fs::File::open(&fixture.saves).unwrap();
        let socket_path = format!("/proc/self/fd/{}/socket", dir.as_raw_fd());
        let _socket = std::os::unix::net::UnixListener::bind(socket_path).unwrap();
        assert!(restore(&fixture.game, &fixture.paths, &fixture.config, &first.id).is_err());
        assert_eq!(
            std::fs::read(fixture.saves.join("slot/niveau1.sav")).unwrap(),
            b"current"
        );
    }

    #[test]
    fn corrupted_snapshot_and_interrupted_restore_never_replace_current_save() {
        let fixture = Fixture::new();
        let first = backup(&fixture.game, &fixture.paths, &fixture.config)
            .unwrap()
            .unwrap();
        std::fs::write(fixture.saves.join("slot/niveau1.sav"), b"current").unwrap();
        std::fs::write(Path::new(&first.path).join("slot/niveau1.sav"), b"corrupt").unwrap();
        assert!(restore(&fixture.game, &fixture.paths, &fixture.config, &first.id).is_err());
        assert_eq!(
            std::fs::read(fixture.saves.join("slot/niveau1.sav")).unwrap(),
            b"current"
        );

        std::fs::write(Path::new(&first.path).join("slot/niveau1.sav"), b"un").unwrap();
        let previous = fixture
            .saves
            .parent()
            .unwrap()
            .join(format!(".lapinux-saves-{}-old", first.id));
        std::fs::create_dir_all(&previous).unwrap();
        std::fs::write(previous.join("recovery"), b"keep").unwrap();
        assert!(restore(&fixture.game, &fixture.paths, &fixture.config, &first.id).is_err());
        assert_eq!(std::fs::read(previous.join("recovery")).unwrap(), b"keep");
        assert_eq!(
            std::fs::read(fixture.saves.join("slot/niveau1.sav")).unwrap(),
            b"current"
        );
    }

    #[test]
    fn verified_backup_rejects_missing_empty_and_destination_inside_game() {
        let mut fixture = Fixture::new();
        let copy = verified_backup(&fixture.game, &fixture.paths, &fixture.config).unwrap();
        assert!(Path::new(&copy).is_dir());
        fixture.config.general.saves_root = fixture
            .game
            .install_dir()
            .join("copies")
            .display()
            .to_string();
        assert!(verified_backup(&fixture.game, &fixture.paths, &fixture.config).is_err());
        fixture.config.general.saves_root.clear();
        std::os::unix::fs::symlink("slot/niveau1.sav", fixture.saves.join("link")).unwrap();
        assert!(verified_backup(&fixture.game, &fixture.paths, &fixture.config).is_err());
        std::fs::remove_dir_all(&fixture.saves).unwrap();
        assert!(verified_backup(&fixture.game, &fixture.paths, &fixture.config).is_err());
        std::fs::create_dir_all(&fixture.saves).unwrap();
        assert!(verified_backup(&fixture.game, &fixture.paths, &fixture.config).is_err());
    }

    #[test]
    fn a_restore_puts_the_files_back_and_keeps_a_safety_copy() {
        let fixture = Fixture::new();
        let first = backup(&fixture.game, &fixture.paths, &fixture.config)
            .unwrap()
            .unwrap();
        std::fs::write(fixture.saves.join("slot/niveau1.sav"), b"abime").unwrap();
        std::fs::write(fixture.saves.join("en-trop.sav"), b"a effacer").unwrap();
        let restored = restore(&fixture.game, &fixture.paths, &fixture.config, &first.id).unwrap();
        assert_eq!(restored, 2);
        assert_eq!(
            std::fs::read(fixture.saves.join("slot/niveau1.sav")).unwrap(),
            b"un"
        );
        // Le dossier du jeu reprend exactement l'état de la copie : un
        // fichier qui n'était pas dans la copie ne survit pas au chargement.
        assert!(
            !fixture.saves.join("en-trop.sav").exists(),
            "le chargement remplace le dossier, il ne le fusionne pas"
        );
        // L'état abîmé a été copié avant d'être remplacé.
        assert_eq!(
            list(&fixture.game, &fixture.paths, &fixture.config).len(),
            2
        );
        let _ = fixture.dir;
    }

    #[test]
    fn a_restore_follows_the_prefix_when_it_moved() {
        let mut fixture = Fixture::new();
        // Le jeu écrit dans un préfixe Windows, pas dans un dossier à côté de
        // son exécutable.
        let old_prefix = fixture.dir.path().join("ancien/compatdata/42");
        let old_saves = old_prefix.join("pfx/drive_c/users/steamuser/AppData/Local/Test");
        std::fs::create_dir_all(&old_saves).unwrap();
        std::fs::write(old_saves.join("partie.sav"), b"progression").unwrap();
        fixture.game.settings.save_dir = Some(old_saves.display().to_string());
        let copy = backup(&fixture.game, &fixture.paths, &fixture.config)
            .unwrap()
            .unwrap();
        assert_eq!(copy.source, old_saves.display().to_string());
        // Le préfixe déménage : le chemin noté n'est plus celui que le jeu
        // relit, même s'il ressemble à une sauvegarde.
        let new_prefix = fixture.dir.path().join("nouveau/compatdata/42");
        let new_saves = new_prefix.join("pfx/drive_c/users/steamuser/AppData/Local/Test");
        std::fs::create_dir_all(&new_saves).unwrap();
        std::fs::write(new_saves.join("partie.sav"), b"abime").unwrap();
        std::fs::remove_dir_all(&old_prefix).unwrap();
        fixture.game.prefix = Some(new_prefix);
        fixture.game.settings.save_dir = Some(new_saves.display().to_string());
        let restored = restore(&fixture.game, &fixture.paths, &fixture.config, &copy.id).unwrap();
        assert_eq!(restored, 1);
        assert_eq!(
            std::fs::read(new_saves.join("partie.sav")).unwrap(),
            b"progression",
            "la copie doit atterrir dans le préfixe que le jeu lit"
        );
        assert!(
            !old_saves.exists(),
            "l'ancien préfixe ne doit pas être recréé"
        );
        let _ = fixture.dir;
    }

    #[test]
    fn restoring_a_copy_keeps_it_while_taking_the_safety_copy() {
        let fixture = Fixture::new();
        for step in 0..3 {
            std::fs::write(
                fixture.saves.join("slot/niveau1.sav"),
                format!("version {step}"),
            )
            .unwrap();
            backup(&fixture.game, &fixture.paths, &fixture.config).unwrap();
        }
        let copies = list(&fixture.game, &fixture.paths, &fixture.config);
        assert_eq!(copies.len(), 3);
        let source = copies[0].clone();
        std::fs::write(fixture.saves.join("slot/niveau1.sav"), b"abime").unwrap();
        restore(&fixture.game, &fixture.paths, &fixture.config, &source.id).unwrap();
        let after = list(&fixture.game, &fixture.paths, &fixture.config);
        assert!(
            after.iter().any(|copy| copy.id == source.id),
            "la copie relue ne doit pas être la place remplacée par la copie de sécurité"
        );
        assert!(game_root(&fixture.game, &fixture.paths, &fixture.config)
            .join(&source.id)
            .is_dir());
        let _ = fixture.dir;
    }

    #[test]
    fn removing_a_copy_deletes_only_it() {
        let fixture = Fixture::new();
        let first = backup(&fixture.game, &fixture.paths, &fixture.config)
            .unwrap()
            .unwrap();
        std::fs::write(fixture.saves.join("slot/niveau1.sav"), b"suivante").unwrap();
        let second = backup(&fixture.game, &fixture.paths, &fixture.config)
            .unwrap()
            .unwrap();
        assert_eq!(
            list(&fixture.game, &fixture.paths, &fixture.config).len(),
            2
        );
        let removed = remove(&fixture.game, &fixture.paths, &fixture.config, &first.id).unwrap();
        assert_eq!(removed.id, first.id);
        let dir = game_root(&fixture.game, &fixture.paths, &fixture.config);
        assert!(!dir.join(&first.id).exists());
        assert!(!dir.join(format!("{}.json", first.id)).exists());
        let left = list(&fixture.game, &fixture.paths, &fixture.config);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, second.id);
        assert!(remove(&fixture.game, &fixture.paths, &fixture.config, "inconnu").is_err());
        let _ = fixture.dir;
    }

    #[test]
    fn an_export_copies_everything_and_leaves_the_original() {
        let fixture = Fixture::new();
        let snapshot = backup(&fixture.game, &fixture.paths, &fixture.config)
            .unwrap()
            .unwrap();
        let dest = fixture.dir.path().join("export");
        let written = export(
            &fixture.game,
            &fixture.paths,
            &fixture.config,
            &dest.display().to_string(),
        )
        .unwrap();
        assert_eq!(written, dest.join("lapinux-saves-42"));
        assert!(written
            .join(&snapshot.id)
            .join("slot/niveau1.sav")
            .is_file());
        assert!(game_root(&fixture.game, &fixture.paths, &fixture.config)
            .join(&snapshot.id)
            .is_dir());
    }

    #[test]
    fn the_configured_save_folder_wins() {
        let mut fixture = Fixture::new();
        let elsewhere = fixture.dir.path().join("autres-sauvegardes");
        std::fs::create_dir_all(&elsewhere).unwrap();
        fixture.game.settings.save_dir = Some(elsewhere.display().to_string());
        assert_eq!(
            source(&fixture.game, &fixture.paths),
            Some(elsewhere.clone())
        );
        // Un chemin relatif part du dossier du jeu.
        fixture.game.settings.save_dir = Some("sauvegardes".into());
        assert_eq!(
            source(&fixture.game, &fixture.paths),
            Some(fixture.saves.clone())
        );
        let _ = fixture.dir;
    }

    #[test]
    fn a_prefix_saved_games_folder_is_found() {
        let mut fixture = Fixture::new();
        // Sans dossier évident à côté du jeu, la détection regarde le préfixe.
        std::fs::remove_dir_all(&fixture.saves).unwrap();
        let prefix = fixture.dir.path().join("prefixes/42");
        let saved = prefix.join("pfx/drive_c/users/steamuser/Saved Games/Test");
        std::fs::create_dir_all(&saved).unwrap();
        std::fs::write(saved.join("slot.sav"), b"x").unwrap();
        fixture.game.prefix = Some(prefix.clone());
        fixture.game.origin = GameOrigin::Lapinux;
        // Le sous-dossier au nom du jeu, plus étroit que tout « Saved
        // Games » : c'est là que ce jeu écrit, et rien d'autre n'y entre.
        assert_eq!(source(&fixture.game, &fixture.paths), Some(saved));
        let _ = fixture.dir;
    }

    #[test]
    fn the_journal_keeps_the_latest_operations_newest_first() {
        let fixture = Fixture::new();
        let store =
            lapinux_core::store::Store::open(&fixture.paths.data_dir.join("test.redb")).unwrap();
        for step in 0..HISTORY_MAX + 2 {
            note(
                &store,
                42,
                lapinux_core::model::SaveEvent {
                    at: step as i64,
                    kind: lapinux_core::model::SaveEventKind::Backup,
                    files: 1,
                    ..Default::default()
                },
            );
        }
        let journal = events(&store, 42);
        assert_eq!(journal.len(), HISTORY_MAX, "le journal reste borné");
        assert_eq!(journal[0].at, (HISTORY_MAX + 1) as i64);
        assert_eq!(journal.last().unwrap().at, 2);
        // Un autre jeu garde son propre journal.
        assert!(events(&store, 43).is_empty());
    }

    #[test]
    fn copies_older_than_the_journal_still_appear_in_the_history() {
        let fixture = Fixture::new();
        let store =
            lapinux_core::store::Store::open(&fixture.paths.data_dir.join("test.redb")).unwrap();
        let snapshot = backup(&fixture.game, &fixture.paths, &fixture.config)
            .unwrap()
            .unwrap();
        let copies = list(&fixture.game, &fixture.paths, &fixture.config);
        let journal = history(&store, 42, &copies);
        assert_eq!(journal.len(), 1, "une copie sans événement reste listée");
        assert!(journal[0].imported);
        assert_eq!(journal[0].snapshot, snapshot.id);
        // Une fois l'opération enregistrée, la ligne reconstituée disparaît.
        note(
            &store,
            42,
            lapinux_core::model::SaveEvent {
                at: snapshot.created,
                kind: lapinux_core::model::SaveEventKind::Backup,
                snapshot: snapshot.id.clone(),
                files: snapshot.files,
                bytes: snapshot.bytes,
                ..Default::default()
            },
        );
        let journal = history(&store, 42, &copies);
        assert_eq!(journal.len(), 1);
        assert!(!journal[0].imported);
    }

    #[test]
    fn a_game_without_any_save_folder_has_no_backup() {
        let fixture = Fixture::new();
        std::fs::remove_dir_all(&fixture.saves).unwrap();
        assert!(source(&fixture.game, &fixture.paths).is_none());
        assert!(backup(&fixture.game, &fixture.paths, &fixture.config)
            .unwrap()
            .is_none());
        let _ = fixture.dir;
    }
}
