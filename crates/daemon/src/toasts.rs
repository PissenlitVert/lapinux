//! Bandeaux de succès : le daemon surveille les déblocages d'une partie et
//! pose une notification en haut de l'écran, au-dessus du jeu.
//!
//! La surveillance relit les succès du jeu à intervalle régulier, sans passer
//! par le cache de la fiche (cinq minutes) : un succès obtenu pendant la
//! partie doit s'annoncer dans la minute, pas à la prochaine ouverture de la
//! fiche. Les icônes sont téléchargées une fois dans le cache, pour que le
//! processus du bandeau n'ait jamais besoin du réseau.

use crate::launch::{hud_output, overlay_exe};
use crate::{Result, Shared};
use lapinux_core::model::{AchievementToast, SteamAchievement, SteamAchievements};
use lapinux_core::stores::Store;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

/// Cadence de relecture des succès pendant une partie : assez rapprochée pour
/// suivre le déblocage, assez lente pour ne pas marteler la session Steam ni
/// les API des boutiques.
const POLL: Duration = Duration::from_secs(45);
/// Temps d'affichage d'un bandeau, avant que le suivant prenne sa place.
const HOLD: Duration = Duration::from_secs(6);
/// Bandeaux en attente au plus : une partie qui débloque vingt succès d'un
/// coup ne doit pas occuper l'écran pendant deux minutes.
const QUEUE_MAX: usize = 4;

/// File d'attente des bandeaux : un seul s'affiche à la fois, les suivants
/// attendent sa sortie de l'écran.
#[derive(Default)]
pub(crate) struct ToastQueue {
    pending: Vec<AchievementToast>,
    /// Un fil vide la file : il n'y en a jamais deux.
    running: bool,
}

impl Shared {
    /// Annonce un succès. Deux succès débloqués ensemble s'affichent l'un
    /// après l'autre, jamais superposés.
    pub(crate) fn announce_achievement(self: &Arc<Self>, toast: AchievementToast) {
        if toast.title.trim().is_empty() {
            return;
        }
        let start = match self.toasts.lock() {
            Ok(mut queue) => {
                if queue.pending.len() >= QUEUE_MAX {
                    tracing::debug!(
                        "bandeau de succès : file pleine, « {} » ignoré",
                        toast.title
                    );
                    return;
                }
                queue.pending.push(toast);
                if queue.running {
                    return;
                }
                queue.running = true;
                true
            }
            Err(_) => return,
        };
        if start {
            let shared = Arc::clone(self);
            std::thread::spawn(move || shared.drain_toasts());
        }
    }

    /// Affiche les bandeaux en attente, un par un, en attendant chaque sortie
    /// avant de lancer le suivant.
    fn drain_toasts(self: &Arc<Self>) {
        loop {
            let next = match self.toasts.lock() {
                Ok(mut queue) if queue.pending.is_empty() => {
                    queue.running = false;
                    None
                }
                Ok(mut queue) => Some(queue.pending.remove(0)),
                Err(_) => None,
            };
            let Some(toast) = next else {
                return;
            };
            if let Err(err) = self.show_toast(&toast) {
                tracing::warn!("bandeau de succès : {err}");
            }
        }
    }

    /// Lance le processus du bandeau et attend sa fin : c'est cette attente
    /// qui sérialise les notifications.
    fn show_toast(&self, toast: &AchievementToast) -> Result<()> {
        let exe = overlay_exe()?;
        let mut command = Command::new(exe);
        command
            .arg("overlay")
            .arg("--achievement-toast")
            .arg("--toast-title")
            .arg(&toast.title)
            .arg("--toast-store")
            .arg(toast.store.key())
            .arg("--toast-hold")
            .arg(HOLD.as_secs().to_string());
        if !toast.game.trim().is_empty() {
            command.arg("--toast-game").arg(&toast.game);
        }
        if let Some(percent) = toast.percent {
            command.arg("--toast-percent").arg(format!("{percent:.1}"));
        }
        if let Some(icon) = &toast.icon {
            command.arg("--toast-icon").arg(icon);
        }
        if let Some(icon) = &toast.game_icon {
            command.arg("--toast-game-icon").arg(icon);
        }
        if let Some(output) = self.toast_output() {
            command.arg("--output").arg(output);
        }
        let log = crate::logs::open_shared(&self.paths, "overlay.log", "bandeau")
            .map_err(lapinux_core::Error::Io)?;
        let log_err = log.try_clone().map_err(lapinux_core::Error::Io)?;
        let mut child = command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::from(log))
            .stderr(std::process::Stdio::from(log_err))
            .spawn()
            .map_err(|e| lapinux_core::Error::Other(format!("bandeau de succès : {e}")))?;
        // Le processus est jetable : un compositeur muet ne doit pas bloquer
        // la file des bandeaux, on le tue passé le temps d'affichage.
        let deadline = std::time::Instant::now() + HOLD + Duration::from_secs(6);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        tracing::warn!("bandeau de succès : arrêté avec {status}");
                    }
                    break;
                }
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(40));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
            }
        }
        Ok(())
    }

    /// Écran du bandeau : celui du jeu quand une partie tourne, sinon celui
    /// des réglages. Sans choix, le compositeur prend sa sortie par défaut.
    fn toast_output(&self) -> Option<String> {
        let config = self.config_snapshot();
        if let Some(info) = self.session_info() {
            return hud_output(&info, &config.launch.output);
        }
        let configured = config.launch.output.trim();
        (!configured.is_empty()).then(|| configured.to_string())
    }

    /// Icône du succès, téléchargée une fois et gardée en PNG dans le cache :
    /// le processus du bandeau n'a pas de réseau, il ne lit que des fichiers.
    pub(crate) fn toast_icon(&self, url: &str) -> Option<std::path::PathBuf> {
        let url = url.trim();
        if url.is_empty() {
            return None;
        }
        let dir = self.paths.cache_dir.join("succes").join("toasts");
        std::fs::create_dir_all(&dir).ok()?;
        let digest = Sha256::digest(url.as_bytes());
        let key: String = digest[..10]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let path = dir.join(format!("{key}.png"));
        if path.is_file() {
            return Some(path);
        }
        let response = ureq::get(url)
            .set("User-Agent", "Lapinux/0.1")
            .timeout(Duration::from_secs(8))
            .call()
            .ok()?;
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut response.into_reader(), &mut bytes).ok()?;
        // Ré-encodée en PNG : l'icône vient en JPEG, en PNG ou en WebP selon
        // la boutique, et le rendu de l'overlay ne lit que deux formats.
        let icon = image::load_from_memory(&bytes).ok()?.to_rgba8();
        icon.save(&path).ok()?;
        Some(path)
    }

    /// Nom du jeu annoncé : la base locale ne connaît que les jeux installés,
    /// la bibliothèque de la boutique garde les autres.
    fn toast_game_name(&self, appid: u32) -> String {
        self.store
            .game(appid)
            .ok()
            .flatten()
            .map(|game| game.name)
            .or_else(|| self.cached_store_entry(appid).map(|entry| entry.name))
            .unwrap_or_default()
    }

    /// Icône du jeu : le cache local d'abord, sinon le visuel publié par la
    /// boutique, téléchargé une fois comme les icônes de succès.
    fn toast_game_icon(&self, appid: u32) -> Option<std::path::PathBuf> {
        crate::tray::menu_icon_file(&self.paths, appid).or_else(|| {
            self.cached_store_entry(appid)
                .map(|entry| entry.cover)
                .filter(|cover| !cover.trim().is_empty())
                .and_then(|cover| self.toast_icon(&cover))
        })
    }

    /// Surveille les succès du jeu lancé : chaque succès qui passe à « obtenu »
    /// devient un bandeau. La surveillance s'arrête avec la session.
    pub(crate) fn watch_achievement_unlocks(self: &Arc<Self>, appid: u32, started: i64) {
        let shared = Arc::clone(self);
        std::thread::spawn(move || shared.follow_achievements(appid, started));
    }

    fn follow_achievements(self: &Arc<Self>, appid: u32, started: i64) {
        let store = crate::store_kind(self, appid).unwrap_or(Store::Steam);
        // itch.io ne publie aucun succès : rien à surveiller, rien à relire.
        if store == Store::Itch {
            return;
        }
        let first = match self.achievements_now(appid, store) {
            Ok(read) => read,
            Err(error) => {
                tracing::debug!("bandeau de succès : surveillance impossible ({error})");
                return;
            }
        };
        if first.items.is_empty() {
            return;
        }
        let mut known: HashSet<String> = first
            .items
            .iter()
            .filter(|item| item.achieved)
            .map(|item| item.api_name.clone())
            .collect();
        let game = self.toast_game_name(appid);
        loop {
            std::thread::sleep(POLL);
            if !self
                .session_info()
                .is_some_and(|info| info.appid == appid && info.started == started)
            {
                return;
            }
            let read = match self.achievements_now(appid, store) {
                Ok(read) => read,
                Err(error) => {
                    tracing::debug!("bandeau de succès : relecture ({error})");
                    continue;
                }
            };
            for item in fresh_unlocks(&mut known, &read, started) {
                self.announce_achievement(AchievementToast {
                    title: item.name.clone(),
                    game: game.clone(),
                    store,
                    percent: item.percent,
                    icon: self.toast_icon(&item.icon),
                    game_icon: self.toast_game_icon(appid),
                });
            }
        }
    }

    /// Lecture sans cache : Steam par sa session, Epic et GOG par leur API.
    fn achievements_now(&self, appid: u32, store: Store) -> Result<SteamAchievements> {
        if store == Store::Steam {
            let read = self.read_steam_achievements_now(appid);
            if read.items.is_empty() {
                let why = read
                    .notes
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "ce jeu ne publie pas de succès".into());
                return Err(lapinux_core::Error::Other(why));
            }
            Ok(read)
        } else {
            self.read_store_achievements_now(appid)
        }
    }

    /// Affiche le bandeau d'un succès du jeu, comme s'il venait d'être
    /// débloqué : c'est le bouton de test de la liste complète. Sans
    /// `api_name`, le dernier succès débloqué est choisi.
    pub(crate) fn test_achievement_toast(
        self: &Arc<Self>,
        appid: u32,
        api_name: &str,
    ) -> Result<serde_json::Value> {
        let store = crate::store_kind(self, appid).unwrap_or(Store::Steam);
        let envelope = if store == Store::Steam {
            self.game_achievements(appid)?
        } else {
            self.store_achievements(appid)?
        };
        let read: SteamAchievements = envelope
            .into_data()
            .map_err(|error| lapinux_core::Error::Other(format!("succès illisibles : {error}")))?;
        let chosen = read
            .items
            .iter()
            .find(|item| !api_name.is_empty() && item.api_name == api_name)
            .or_else(|| {
                read.items
                    .iter()
                    .filter(|item| item.achieved)
                    .max_by_key(|item| item.unlock_time.unwrap_or(0))
            })
            .or_else(|| read.items.first())
            .ok_or_else(|| lapinux_core::Error::Other("ce jeu ne publie aucun succès".into()))?;
        let game = self.toast_game_name(appid);
        let toast = AchievementToast {
            title: chosen.name.clone(),
            game,
            store,
            percent: chosen.percent,
            icon: self.toast_icon(&chosen.icon),
            game_icon: self.toast_game_icon(appid),
        };
        let title = toast.title.clone();
        self.announce_achievement(toast);
        Ok(serde_json::json!({ "title": title }))
    }
}

/// Succès devenus « obtenus » depuis le relevé précédent. `known` est mis à
/// jour : un succès déjà vu ne reclame jamais. Un succès daté d'avant la partie
/// (Steam ou la boutique le synchronise en retard) n'est pas une nouvelle de
/// cette session.
fn fresh_unlocks<'a>(
    known: &mut HashSet<String>,
    read: &'a SteamAchievements,
    started: i64,
) -> Vec<&'a SteamAchievement> {
    let mut found = Vec::new();
    for item in &read.items {
        if !item.achieved {
            continue;
        }
        if !known.insert(item.api_name.clone()) {
            continue;
        }
        if item.unlock_time.is_some_and(|time| time < started) {
            continue;
        }
        found.push(item);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn achievement(api_name: &str, achieved: bool, unlock_time: Option<i64>) -> SteamAchievement {
        SteamAchievement {
            api_name: api_name.into(),
            name: format!("Succès {api_name}"),
            achieved,
            unlock_time,
            percent: Some(12.5),
            ..Default::default()
        }
    }

    #[test]
    fn only_new_unlocks_are_announced() {
        let started = 1_000;
        let first = SteamAchievements {
            items: vec![
                achievement("DEJA", true, Some(900)),
                achievement("LOCK", false, None),
            ],
            ..Default::default()
        };
        let mut known: HashSet<String> = first
            .items
            .iter()
            .filter(|item| item.achieved)
            .map(|item| item.api_name.clone())
            .collect();
        // Rien de neuf : aucune notification.
        assert!(fresh_unlocks(&mut known, &first, started).is_empty());
        let second = SteamAchievements {
            items: vec![
                achievement("DEJA", true, Some(900)),
                achievement("LOCK", false, None),
                achievement("NOUVEAU", true, Some(1_100)),
                // Débloqué avant la partie, synchronisé seulement maintenant :
                // ce n'est pas une nouvelle de la session.
                achievement("VIEUX", true, Some(500)),
            ],
            ..Default::default()
        };
        let found = fresh_unlocks(&mut known, &second, started);
        let names: Vec<&str> = found.iter().map(|item| item.api_name.as_str()).collect();
        assert_eq!(names, vec!["NOUVEAU"]);
        // Le succès vu une fois ne revient pas au relevé suivant.
        assert!(fresh_unlocks(&mut known, &second, started).is_empty());
    }

    #[test]
    fn an_undated_unlock_is_announced() {
        let mut known = HashSet::new();
        let read = SteamAchievements {
            items: vec![achievement("SANS_DATE", true, None)],
            ..Default::default()
        };
        assert_eq!(fresh_unlocks(&mut known, &read, 1_000).len(), 1);
    }
}
