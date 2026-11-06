//! Partie sociale Steam du daemon : une session permanente qui publie la
//! présence, apporte les amis, la famille et les notifications, et entretient
//! le jeton de refresh.
//!
//! L'interface ne parle jamais directement au réseau Steam : elle demande
//! [`Req::SteamSocial`](lapinux_core::ipc::Req::SteamSocial) et change l'état
//! publié par `SteamSocialPresence`. La session vit tant que le daemon tourne,
//! comme le client de Valve : fermer la fenêtre ne fait pas disparaître le
//! compte de la liste d'amis.

use crate::{Result, Shared};
use lapinux_core::ipc::Envelope;
use lapinux_core::social::{SteamFriend, SteamFriendsState, SteamPresence, SteamSocial};

use lapinux_steam::session;
use lapinux_steam::social::Social;
use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// Avatars téléchargés simultanément au maximum : une grande liste se remplit
/// en quelques secondes sans lancer cent requêtes d'un coup.
const AVATAR_CONCURRENCY: usize = 12;
static AVATAR_ACTIVE: AtomicUsize = AtomicUsize::new(0);
/// Un avatar Steam fait quelques kilo-octets ; cette borne protège le cache
/// d'une réponse inattendue.
const AVATAR_MAX_BYTES: u64 = 2 * 1024 * 1024;

fn avatar_attempts() -> &'static Mutex<HashSet<u64>> {
    static ATTEMPTS: OnceLock<Mutex<HashSet<u64>>> = OnceLock::new();
    ATTEMPTS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Seules les images servies par Steam sont mises en cache.
fn is_steam_avatar_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split('/').next().unwrap_or_default();
    host == "steamstatic.com" || host.ends_with(".steamstatic.com")
}

fn download_avatar(url: &str, path: &Path) {
    let downloaded = (|| -> Option<()> {
        let response = ureq::get(url)
            .set("User-Agent", "Lapinux/0.1 (cache d'avatars Steam)")
            .timeout(Duration::from_secs(8))
            .call()
            .ok()?;
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(AVATAR_MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.is_empty() || bytes.len() as u64 > AVATAR_MAX_BYTES {
            return None;
        }
        let temporary = path.with_extension("part");
        std::fs::write(&temporary, &bytes).ok()?;
        std::fs::rename(&temporary, path).ok()?;
        Some(())
    })();
    if downloaded.is_none() {
        let _ = std::fs::remove_file(path.with_extension("part"));
    }
}

impl Shared {
    /// Chemins des avatars déjà en cache, et téléchargement en tâche de fond
    /// des suivants. Une seule tentative par ami et par vie du daemon évite de
    /// marteler Steam si une image manque.
    fn refresh_friend_avatars(&self, friends: &[SteamFriend]) -> BTreeMap<u64, PathBuf> {
        let directory = self.paths.cache_dir.join("steam-avatars");
        if std::fs::create_dir_all(&directory).is_err() {
            return BTreeMap::new();
        }
        let mut queued = Vec::new();
        let slots = AVATAR_CONCURRENCY.saturating_sub(AVATAR_ACTIVE.load(Ordering::Relaxed));
        if slots > 0 {
            if let Ok(mut attempts) = avatar_attempts().lock() {
                for friend in friends {
                    if !is_steam_avatar_url(&friend.avatar) {
                        continue;
                    }
                    let path = directory.join(format!("{}.jpg", friend.steam_id));
                    if path.is_file() || !attempts.insert(friend.steam_id) {
                        continue;
                    }
                    queued.push((friend.avatar.clone(), path));
                    if queued.len() >= slots {
                        break;
                    }
                }
            }
        }
        if !queued.is_empty() {
            AVATAR_ACTIVE.fetch_add(queued.len(), Ordering::Relaxed);
        }
        for (url, path) in queued {
            std::thread::spawn(move || {
                download_avatar(&url, &path);
                AVATAR_ACTIVE.fetch_sub(1, Ordering::Relaxed);
            });
        }
        friends
            .iter()
            .filter_map(|friend| {
                let path = directory.join(format!("{}.jpg", friend.steam_id));
                path.is_file().then_some((friend.steam_id, path))
            })
            .collect()
    }

    /// Démarre la session sociale si un jeton est enregistré et si la
    /// configuration ne la désactive pas. Idempotent.
    pub fn ensure_social(self: &Arc<Self>) -> Result<Arc<Social>> {
        if let Ok(slot) = self.social.lock() {
            if let Some(social) = slot.as_ref() {
                return Ok(Arc::clone(social));
            }
        }
        if self.social_paused.load(Ordering::SeqCst) {
            return Err(lapinux_core::Error::Other(
                "session sociale en pause : Steam ou synchronisation du catalogue en cours".into(),
            ));
        }
        let config = self.config_snapshot();
        if !config.steam.social {
            return Err(lapinux_core::Error::Other(
                "session sociale désactivée dans les réglages".into(),
            ));
        }
        let saved = session::load_from(&self.session_path())
            .filter(|saved| !saved.refresh_token.trim().is_empty())
            .ok_or_else(|| {
                lapinux_core::Error::Other(
                    "connectez d'abord votre compte Steam pour apparaître en ligne".into(),
                )
            })?;
        // Une session qui naît pendant une partie doit reprendre l'état
        // « en jeu » : sinon l'activité disparaît à chaque reconnexion.
        let playing = if config.steam.show_playing {
            self.steam_playing.lock().ok().and_then(|slot| *slot)
        } else {
            None
        };
        let playing_name = self.session_info().map(|info| info.name);
        let social = Arc::new(Social::start_with_playing(
            saved,
            config.steam.presence,
            playing,
            playing_name,
        )?);
        if let Ok(mut slot) = self.social.lock() {
            *slot = Some(Arc::clone(&social));
        }
        Ok(social)
    }

    /// État social : l'interface y lit les amis, la famille et les
    /// notifications. `refresh` redemande la liste d'amis.
    pub fn steam_social(self: &Arc<Self>, refresh: bool) -> Result<Envelope> {
        if self.social_paused.load(Ordering::SeqCst) {
            // La session sociale reprend dès la fin du catalogue : l'interface
            // voit l'état « connexion », comme pendant une reconnexion.
            return Ok(Envelope::ok(&SteamSocial::connecting()));
        }
        let social = match self.ensure_social() {
            Ok(social) => social,
            Err(error) => {
                return Ok(Envelope::ok(&SteamSocial::failed(error.to_string())));
            }
        };
        if refresh {
            social.refresh();
        }
        let mut snapshot = social.snapshot();
        self.name_friends_games(&mut snapshot.friends);
        Ok(Envelope::ok(&snapshot))
    }

    /// Liste d'amis, avatars et discussions de l'overlay. Elle ne dépend ni
    /// d'un jeu lancé ni de ses catégories : discuter avec un ami reste
    /// possible pendant n'importe quelle partie.
    pub fn steam_friends_state(self: &Arc<Self>) -> Result<Envelope> {
        let social = match self.ensure_social() {
            Ok(social) => social,
            Err(error) => {
                let state = SteamFriendsState {
                    error: Some(error.to_string()),
                    ..SteamFriendsState::default()
                };
                return Ok(Envelope::ok(&state));
            }
        };
        let snapshot = social.snapshot();
        let live = snapshot.live();
        let self_id = snapshot.steam_id;
        let mut friends: Vec<SteamFriend> = snapshot
            .friends
            .into_iter()
            .filter(|friend| friend.steam_id != 0 && friend.steam_id != self_id)
            .collect();
        friends.sort_by(|a, b| {
            b.in_game()
                .cmp(&a.in_game())
                .then_with(|| b.presence.online().cmp(&a.presence.online()))
                .then_with(|| {
                    a.name
                        .to_lowercase()
                        .cmp(&b.name.to_lowercase())
                        .then(a.steam_id.cmp(&b.steam_id))
                })
        });
        let avatars = self.refresh_friend_avatars(&friends);
        let state = SteamFriendsState {
            live,
            error: snapshot.error,
            friends,
            avatars,
            chats: snapshot.chats,
        };
        Ok(Envelope::ok(&state))
    }

    /// Active une clé produit sur le compte connecté.
    ///
    /// La tentative entre dans l'historique tout de suite : le verdict de
    /// Steam arrive dans `redeem`, que l'interface suit à l'image suivante.
    pub fn steam_social_redeem(self: &Arc<Self>, key: &str) -> Result<Envelope> {
        let key = key.trim();
        if key.is_empty() {
            return Err(lapinux_core::Error::Other(
                "saisissez une clé produit Steam".into(),
            ));
        }
        // Une clé arrive souvent juste après la précédente : la relève de
        // celle-là met la session sociale en pause, et la session suivante met
        // une seconde à se connecter. Envoyée dans cet intervalle, la demande
        // tombait sur une session fermée et la seconde activation ne faisait
        // rien. La pause se lève d'elle-même : on lui laisse un demi-seconde,
        // puis on la lève nous-mêmes.
        let started = std::time::Instant::now();
        let deadline = started + Duration::from_secs(2);
        let mut resumed = started;
        loop {
            if self.social_paused.load(Ordering::SeqCst)
                && started.elapsed() > Duration::from_millis(600)
                && resumed.elapsed() > Duration::from_millis(600)
            {
                self.resume_social();
                resumed = std::time::Instant::now();
            }
            // La clé part dans la file du fil social : il l'envoie tout de
            // suite s'il est connecté, sinon il la garde pour la connexion
            // suivante, une fois la session en place.
            let social = self.ensure_social();
            if let Ok(social) = &social {
                if social.redeem(key) {
                    tracing::info!("activation : clé remise au fil social");
                    return Ok(Envelope::ok(&social.snapshot()));
                }
            }
            if std::time::Instant::now() >= deadline {
                return match &social {
                    Ok(social) => {
                        tracing::warn!(
                            "activation : fil social fermé ({})",
                            social.snapshot().state
                        );
                        Ok(Envelope::ok(&SteamSocial::failed(
                            "la session Steam se remet en place : relancez l'activation dans un instant"
                                .to_string(),
                        )))
                    }
                    Err(error) => {
                        tracing::warn!("activation : {error}");
                        Ok(Envelope::ok(&SteamSocial::failed(error.to_string())))
                    }
                };
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Envoie un message texte à un ami par la session sociale.
    pub fn steam_social_message(
        self: &Arc<Self>,
        steam_id: u64,
        text: &str,
    ) -> Result<Envelope> {
        if steam_id == 0 {
            return Err(lapinux_core::Error::Other(
                "SteamID du destinataire manquant".into(),
            ));
        }
        let text = text.trim();
        if text.is_empty() {
            return Err(lapinux_core::Error::Other("message vide".into()));
        }
        if text.len() > 1024 {
            return Err(lapinux_core::Error::Other("message trop long".into()));
        }
        let social = self.ensure_social().map_err(|error| {
            lapinux_core::Error::Other(format!("session sociale indisponible : {error}"))
        })?;
        let snapshot = social.snapshot();
        if !snapshot.live() {
            return Err(lapinux_core::Error::Other(
                "session sociale pas encore connectée ; réessayez dans un instant".into(),
            ));
        }
        if !snapshot
            .friends
            .iter()
            .any(|friend| friend.steam_id == steam_id)
        {
            return Err(lapinux_core::Error::Other(
                "ce compte n'est pas dans votre liste d'amis Steam".into(),
            ));
        }
        if !social.message(steam_id, text) {
            return Err(lapinux_core::Error::Other(
                "la session sociale n'accepte plus de commandes".into(),
            ));
        }
        Ok(Envelope::ok(&serde_json::json!({
            "steam_id": steam_id,
            "transmitted": true,
        })))
    }

    /// Complète le nom du jeu des amis qui jouent.
    ///
    /// Steam ne publie pas toujours ce nom : la présence riche ne le porte que
    /// si le jeu l'écrit. L'appid, lui, est toujours là — le catalogue donne le
    /// nom des jeux possédés, la boutique publique celui des autres.
    fn name_friends_games(self: &Arc<Self>, friends: &mut [lapinux_core::social::SteamFriend]) {
        let playing: Vec<usize> = friends
            .iter()
            .enumerate()
            .filter(|(_, friend)| friend.game.trim().is_empty() && friend.game_appid != 0)
            .map(|(index, _)| index)
            .collect();
        if playing.is_empty() {
            return;
        }
        let catalog = lapinux_steam::catalog::Catalog::load(
            &self.paths.data_dir.join("steam-catalog.json"),
        );
        for index in playing {
            let appid = friends[index].game_appid;
            if let Some(entry) = catalog.get(appid) {
                friends[index].game = entry.name.clone();
                continue;
            }
            if let Some(name) = lapinux_steam::catalog::Catalog::store_name(appid) {
                friends[index].game = name;
            }
        }
    }

    /// Publie le jeu Steam en cours, ou efface l'état avec `None`.
    ///
    /// L'état est gardé même quand la session sociale n'est pas encore née :
    /// une reconnexion ou une pause du catalogue le reprendra.
    pub fn set_steam_playing(self: &Arc<Self>, appid: Option<u32>) {
        if let Ok(mut slot) = self.steam_playing.lock() {
            *slot = appid;
        }
        let config = self.config_snapshot();
        let wanted = if config.steam.show_playing {
            appid
        } else {
            None
        };
        let name = self.session_info().map(|info| info.name);
        if let Ok(slot) = self.social.lock() {
            if let Some(social) = slot.as_ref() {
                social.set_playing(wanted, name.as_deref());
                return;
            }
        }
        // Sans session vivante, inutile d'en ouvrir une uniquement pour
        // effacer un état : la prochaine session repartira proprement.
        let Some(appid) = wanted else {
            return;
        };
        match self.ensure_social() {
            Ok(social) => {
                social.set_playing(Some(appid), name.as_deref());
            }
            Err(error) => tracing::debug!("jeu en cours non publié sur Steam : {error}"),
        }
    }

    /// Change la présence publiée sur Steam et l'enregistre pour la prochaine
    /// ouverture.
    pub fn steam_social_presence(self: &Arc<Self>, presence: SteamPresence) -> Result<Envelope> {
        let mut config = self.config_snapshot();
        if config.steam.presence != presence {
            config.steam.presence = presence;
            self.save_config(config)?;
        }
        let social = match self.ensure_social() {
            Ok(social) => social,
            Err(error) => {
                return Ok(Envelope::ok(&SteamSocial::failed(error.to_string())));
            }
        };
        social.set_presence(presence);
        let snapshot = social.snapshot();
        Ok(Envelope::ok(&snapshot))
    }

    /// Met la session sociale en pause : Steam ne garde qu'une session par
    /// compte, chaque nouveau logon ferme la précédente, et la session sociale
    /// se reconnecte toute seule huit secondes plus tard — de quoi couper une
    /// synchronisation en plein vol. Rien ne la recrée tant que dure la pause.
    pub fn pause_social(&self) {
        self.social_paused.store(true, Ordering::SeqCst);
        self.stop_social();
    }

    /// Sort de la pause : la session sociale repart dans son fil.
    pub fn resume_social(self: &Arc<Self>) {
        self.social_paused.store(false, Ordering::SeqCst);
        self.start_social_in_background();
    }

    /// Arrête la session sociale (jeton oublié, réglage désactivé).
    pub fn stop_social(&self) {
        if let Ok(mut slot) = self.social.lock() {
            if let Some(social) = slot.take() {
                social.stop();
            }
        }
    }

    /// Démarre la session au lancement du daemon, sans bloquer : les erreurs
    /// réseau n'empêchent pas l'application de fonctionner.
    pub fn start_social_in_background(self: &Arc<Self>) {
        let shared = Arc::clone(self);
        std::thread::spawn(move || {
            if let Err(error) = shared.ensure_social() {
                tracing::debug!("session sociale non démarrée : {error}");
            }
        });
    }

    /// Relance la session après une connexion ou une déconnexion : le jeton a
    /// changé, l'ancienne session ne vaut plus rien.
    pub fn restart_social_in_background(self: &Arc<Self>) {
        self.stop_social();
        self.start_social_in_background();
    }

    /// Applique un changement de configuration à la session vivante : présence
    /// publiée, ou arrêt si la session sociale est désactivée.
    pub fn sync_social(&self) {
        let config = self.config_snapshot();
        if !config.steam.social {
            self.stop_social();
            return;
        }
        let playing = if config.steam.show_playing {
            self.steam_playing.lock().ok().and_then(|slot| *slot)
        } else {
            None
        };
        let name = self.session_info().map(|info| info.name);
        if let Ok(slot) = self.social.lock() {
            if let Some(social) = slot.as_ref() {
                social.set_presence(config.steam.presence);
                social.set_playing(playing, name.as_deref());
                return;
            }
        }
        // Aucune session en vie : elle naîtra au prochain besoin.
    }
}

#[cfg(test)]
mod tests {
    use super::Shared;
    use lapinux_core::paths::LapinuxPaths;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    #[test]
    fn la_session_sociale_ne_renait_pas_pendant_une_synchronisation() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Arc::new(Shared::new(&paths).unwrap());
        shared.pause_social();
        assert!(
            shared.ensure_social().is_err(),
            "aucune session ne doit naître pendant la pause du catalogue"
        );
        shared.resume_social();
        assert!(!shared.social_paused.load(Ordering::SeqCst));
    }

    #[test]
    fn les_avatars_ne_viennent_que_de_steam() {
        assert!(super::is_steam_avatar_url(
            "https://avatars.fastly.steamstatic.com/abc_full.jpg"
        ));
        assert!(super::is_steam_avatar_url(
            "https://avatars.steamstatic.com/abc_full.jpg"
        ));
        assert!(!super::is_steam_avatar_url(
            "http://avatars.fastly.steamstatic.com/abc_full.jpg"
        ));
        assert!(!super::is_steam_avatar_url(
            "https://steamstatic.com.exemple/avatar.jpg"
        ));
        assert!(!super::is_steam_avatar_url(
            "https://exemple.com/abc_full.jpg"
        ));
    }

    #[test]
    fn la_pause_repond_un_etat_social_en_connexion() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Arc::new(Shared::new(&paths).unwrap());
        shared.pause_social();
        let social = shared
            .steam_social(false)
            .unwrap()
            .into_data::<lapinux_core::social::SteamSocial>()
            .unwrap();
        assert!(social.error.is_none(), "la pause n'est pas une erreur");
        assert_eq!(social.state, "connecting");
        shared.resume_social();
    }

}
