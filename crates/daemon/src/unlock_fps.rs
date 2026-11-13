//! Déblocage de la limite de cadence que le jeu s'impose à lui-même.
//!
//! Les jeux Unreal écrivent leur plafond dans `GameUserSettings.ini`
//! (`FrameRateLimit`), relu à chaque démarrage : ni GameScope ni DXVK ne
//! peuvent le relever, ils ne savent que baisser la cadence. L'option par jeu
//! réécrit donc cette ligne avant le lancement et retient la valeur d'origine,
//! pour que la décocher la remette telle quelle.
//!
//! Seule cette ligne est jamais touchée : le reste du fichier garde son texte
//! exact, fins de ligne et réglages graphiques compris.

use std::path::{Path, PathBuf};

use lapinux_core::model::Game;

/// Nom visé, comparé sans la casse : les jeux Windows écrivent
/// `GameUserSettings.ini`, quelques outils `gameusersettings.ini`.
const FILE: &str = "gameusersettings.ini";
const KEY: &str = "FrameRateLimit=";
const UNLOCKED: &str = "FrameRateLimit=0.000000";
/// `Saved/Config/Windows/` tient en quatre niveaux sous `AppData/Local`.
const MAX_DEPTH: usize = 8;

fn folder(data_dir: &Path, appid: u32) -> PathBuf {
    data_dir.join("unlock-fps").join(appid.to_string())
}

/// Fichiers de cadence reconnus : ceux du dossier du jeu, puis ceux du préfixe
/// Proton (`AppData/Local`, `Documents/My Games`).
fn targets(game: &Game, data_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect(&game.install_dir(), 0, &mut files);
    let prefix = lapinux_proton::prefix_for(game, data_dir);
    // Le préfixe moderne range `drive_c` sous `pfx/`, les anciens à sa racine.
    for base in ["drive_c", "pfx/drive_c"] {
        let Ok(users) = std::fs::read_dir(prefix.join(base).join("users")) else {
            continue;
        };
        for user in users.flatten().filter(|entry| entry.path().is_dir()) {
            for relative in ["AppData/Local", "Documents/My Games"] {
                collect(&user.path().join(relative), 0, &mut files);
            }
        }
    }
    files.sort();
    files.dedup();
    files
}

fn collect(dir: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        // Un lien symbolique peut sortir du dossier du jeu ou boucler.
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => collect(&entry.path(), depth + 1, files),
            Ok(kind) if kind.is_file() => {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(FILE)
                {
                    files.push(entry.path());
                }
            }
            _ => {}
        }
    }
}

/// Ligne de cadence telle que le jeu l'a écrite.
fn limit_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| line.starts_with(KEY))
}

/// Le fichier avec sa ligne de cadence remplacée par `value`, ou `None` quand
/// il n'en a pas ou qu'elle porte déjà cette valeur. Le reste est rendu tel
/// quel, fin de ligne de la ligne remplacée comprise.
fn with_limit(text: &str, value: &str) -> Option<String> {
    let mut replaced = false;
    let mut out = String::with_capacity(text.len() + 2);
    for line in text.split_inclusive('\n') {
        let content = line.trim_end_matches(['\r', '\n']);
        let ending = &line[content.len()..];
        if !replaced && content.trim_start().starts_with(KEY) {
            replaced = true;
            if content.trim() == value.trim() {
                return None;
            }
            out.push_str(value);
        } else {
            out.push_str(content);
        }
        out.push_str(ending);
    }
    replaced.then_some(out)
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Entry {
    target: String,
    /// Ligne d'origine, remise telle quelle à la restauration.
    line: String,
}

fn read_manifest(path: &Path) -> Vec<Entry> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Lève la limite des fichiers reconnus et retient la ligne d'origine de
/// chacun. Rend le nombre de fichiers qui portaient une limite (0 : aucun
/// moteur géré trouvé).
pub fn apply(game: &Game, data_dir: &Path) -> Result<usize, String> {
    let manifest = folder(data_dir, game.appid).join("manifest.json");
    let mut entries = read_manifest(&manifest);
    let mut handled = 0;
    for file in targets(game, data_dir) {
        let Ok(text) = std::fs::read_to_string(&file) else {
            // Un fichier UTF-16 (certains jeux Windows) serait corrompu par
            // une réécriture en UTF-8 : il reste tel quel.
            tracing::warn!("limite du jeu : {} illisible, ignoré", file.display());
            continue;
        };
        let Some(line) = limit_line(&text) else {
            continue;
        };
        handled += 1;
        let Some(patched) = with_limit(&text, UNLOCKED) else {
            continue;
        };
        let target = file.to_string_lossy().into_owned();
        if !entries.iter().any(|entry| entry.target == target) {
            std::fs::create_dir_all(folder(data_dir, game.appid))
                .map_err(|error| error.to_string())?;
            entries.push(Entry {
                target,
                line: line.to_string(),
            });
            let _ = std::fs::write(&manifest, serde_json::to_vec(&entries).unwrap_or_default());
        }
        std::fs::write(&file, patched).map_err(|error| error.to_string())?;
    }
    Ok(handled)
}

/// Remet la seule ligne de cadence d'origine et oublie la sauvegarde. Le reste
/// du fichier reste celui du jeu, réglages graphiques compris.
pub fn restore(data_dir: &Path, appid: u32) -> Result<usize, String> {
    let manifest = folder(data_dir, appid).join("manifest.json");
    let Ok(bytes) = std::fs::read(&manifest) else {
        return Ok(0);
    };
    let entries: Vec<Entry> = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    let mut restored = 0;
    for entry in &entries {
        let Ok(text) = std::fs::read_to_string(&entry.target) else {
            // Jeu désinstallé, ou fichier devenu illisible : rien à remettre.
            continue;
        };
        if let Some(original) = with_limit(&text, &entry.line) {
            if std::fs::write(&entry.target, original).is_ok() {
                restored += 1;
            }
        }
    }
    let _ = std::fs::remove_dir_all(folder(data_dir, appid));
    Ok(restored)
}

impl crate::Shared {
    /// Aligne les fichiers du jeu sur l'option et le dit dans le journal.
    /// Appelé quand le réglage change et à chaque lancement ; sans sauvegarde,
    /// décocher n'écrit rien.
    pub(crate) fn sync_unlock_fps(&self, appid: u32) {
        let Ok(Some(game)) = self.store.game(appid) else {
            return;
        };
        let unlock = game.settings.unlock_fps == Some(true);
        let result = if unlock {
            apply(&game, &self.paths.data_dir)
        } else {
            restore(&self.paths.data_dir, appid)
        };
        match (unlock, result) {
            (true, Ok(0)) => {
                tracing::info!("limite du jeu {appid} : aucun fichier de cadence reconnu")
            }
            (true, Ok(count)) => {
                tracing::info!("limite du jeu {appid} débloquée ({count} fichier(s))")
            }
            (true, Err(err)) => tracing::warn!("limite du jeu {appid} : {err}"),
            (false, Ok(count)) if count > 0 => {
                tracing::info!("limite du jeu {appid} : {count} fichier(s) remis d'origine")
            }
            (false, Ok(_)) => {}
            (false, Err(err)) => tracing::warn!("limite du jeu {appid} : retour impossible ({err})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ecrire(dir: &Path, contenu: &str) -> PathBuf {
        let file = dir.join("Saved/Config/Windows/GameUserSettings.ini");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, contenu).unwrap();
        file
    }

    const REGLAGES: &str = "[/Script/Engine.GameUserSettings]\n\
FrameRateLimit=60.000000\n\
bUseVSync=False\n\
ResolutionSizeX=2560\n";

    #[test]
    fn debloquer_puis_remettre_comme_avant() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let file = ecrire(&root.path().join("jeu"), REGLAGES);
        let game = Game {
            appid: 42,
            installdir: root.path().join("jeu").to_string_lossy().into_owned(),
            ..Default::default()
        };
        assert_eq!(apply(&game, &data).unwrap(), 1);
        let patch = std::fs::read_to_string(&file).unwrap();
        assert!(patch.contains("FrameRateLimit=0.000000"));
        // Le reste du fichier ne bouge pas.
        assert!(patch.contains("bUseVSync=False"));
        // Un second passage ne perd pas la ligne d'origine.
        assert_eq!(apply(&game, &data).unwrap(), 1);
        assert_eq!(restore(&data, 42).unwrap(), 1);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), REGLAGES);
        // Restaurer deux fois ne casse rien : le manifeste a disparu.
        assert_eq!(restore(&data, 42).unwrap(), 0);
    }

    #[test]
    fn la_restauration_ne_touche_pas_aux_reglages_changes_entre_temps() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let file = ecrire(&root.path().join("jeu"), REGLAGES);
        let game = Game {
            appid: 12,
            installdir: root.path().join("jeu").to_string_lossy().into_owned(),
            ..Default::default()
        };
        apply(&game, &data).unwrap();
        // Le joueur change sa résolution et sa qualité pendant la partie.
        let modifie = std::fs::read_to_string(&file)
            .unwrap()
            .replace("ResolutionSizeX=2560", "ResolutionSizeX=1920")
            .replace("bUseVSync=False", "bUseVSync=True");
        std::fs::write(&file, &modifie).unwrap();
        restore(&data, 12).unwrap();
        let apres = std::fs::read_to_string(&file).unwrap();
        // Seule la ligne de cadence revient : les nouveaux réglages restent.
        assert!(apres.contains("FrameRateLimit=60.000000"));
        assert!(apres.contains("ResolutionSizeX=1920"));
        assert!(apres.contains("bUseVSync=True"));
    }

    #[test]
    fn les_fins_de_ligne_windows_sont_conservees() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let contenu =
            "[/Script/Engine.GameUserSettings]\r\nFrameRateLimit=144.000000\r\nbUseVSync=True\r\n";
        let file = ecrire(&root.path().join("jeu"), contenu);
        let game = Game {
            appid: 7,
            installdir: root.path().join("jeu").to_string_lossy().into_owned(),
            ..Default::default()
        };
        apply(&game, &data).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "[/Script/Engine.GameUserSettings]\r\nFrameRateLimit=0.000000\r\nbUseVSync=True\r\n"
        );
        restore(&data, 7).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), contenu);
    }

    #[test]
    fn un_fichier_sans_ligne_de_cadence_reste_intact() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let contenu = "[Scalability]\nViewDistance=3\n";
        let file = ecrire(&root.path().join("jeu"), contenu);
        let game = Game {
            appid: 9,
            installdir: root.path().join("jeu").to_string_lossy().into_owned(),
            ..Default::default()
        };
        assert_eq!(apply(&game, &data).unwrap(), 0);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), contenu);
        assert!(!folder(&data, 9).exists());
    }

    #[test]
    fn une_limite_deja_levee_ne_touche_a_rien() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let file = ecrire(
            &root.path().join("jeu"),
            "[/Script/Engine.GameUserSettings]\nFrameRateLimit=0.000000\n",
        );
        let game = Game {
            appid: 3,
            installdir: root.path().join("jeu").to_string_lossy().into_owned(),
            ..Default::default()
        };
        assert_eq!(apply(&game, &data).unwrap(), 1);
        assert!(!folder(&data, 3).exists());
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "[/Script/Engine.GameUserSettings]\nFrameRateLimit=0.000000\n"
        );
    }
}
