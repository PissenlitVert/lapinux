//! Boutiques hors Steam : Epic Games, GOG et itch.io.
//!
//! Chaque boutique a son fichier, mais toutes rendent les mêmes objets : les
//! jetons enregistrés ([`lapinux_core::stores::StoreTokens`]), la bibliothèque
//! du compte ([`StoreGame`]) et un plan d'installation ([`InstallPlan`]). Le
//! daemon et l'interface n'ont donc qu'un point d'entrée, [`login_link`],
//! [`login_code`], [`login_key`], [`library`] et [`install_plan`].
//!
//! Epic et GOG publient aussi les **succès** et les **sauvegardes cloud** :
//! Epic par `datastorage` (`savesync`) et le GraphQL de la boutique, GOG par
//! `cloudstorage.gog.com` et `gameplay.gog.com`. itch.io n'a ni l'un ni l'autre.
//! [`lapinux_core::stores::Store::capabilities`] le dit, l'interface n'affiche
//! pas d'encart vide pour autant.

pub mod achievements;
pub mod cloud;
pub mod common;
pub mod epic;
pub mod epic_manifest;
pub mod gog;
pub mod itch;

pub use achievements::Achievement;
pub use cloud::RemoteFile;
pub use common::{Error, InstallPlan, LoginLink, Result, Step};
pub use lapinux_core::stores::{
    ClaimOutcome, FreeGame, Store, StoreDetails, StoreGame, StoreTokens,
};

use std::path::Path;
use std::sync::Mutex;

/// Le lien de connexion à ouvrir dans le navigateur.
///
/// itch.io n'a pas de flux OAuth : son lien mène à la page où créer la clé
/// d'API que le launcher demande ensuite.
pub fn login_link(store: Store) -> Result<LoginLink> {
    match store {
        Store::Epic => Ok(epic::login_link()),
        Store::Gog => Ok(gog::login_link()),
        Store::Itch => Ok(itch::login_link()),
        Store::Steam => Err(Error::Unavailable("Steam a sa propre connexion")),
    }
}

/// Jeux gratuits publiés par les boutiques : les offres PC en cours et celles
/// annoncées, puis les offres mobiles Android et iOS, que le flux PC ne
/// publie pas. GOG et itch n'en publient pas d'équivalentes.
pub fn free_games(locale: &str) -> Result<Vec<FreeGame>> {
    let mut games = epic::free_games(locale)?;
    // Une offre mobile peut disparaître sans que la page PC échoue : la liste
    // PC reste alors complète, et c'est le seul effet d'une panne mobile.
    if let Ok(mobile) = epic::mobile_free_games(locale) {
        games.extend(mobile);
    }
    // Les offres de la semaine d'abord, celles sans échéance connue ensuite,
    // puis par date de fin et par titre.
    games.sort_by(|left, right| {
        left.upcoming
            .cmp(&right.upcoming)
            .then((left.end == 0).cmp(&(right.end == 0)))
            .then(left.end.cmp(&right.end))
            .then(left.title.cmp(&right.title))
    });
    Ok(games)
}

/// Marque les offres gratuites déjà rattachées au compte, pour que la page
/// les montre comme récupérées. Epic rend ses droits ; les autres boutiques
/// n'ont pas d'offres gratuites.
pub fn mark_owned(tokens: &StoreTokens, games: &mut [FreeGame]) -> Result<()> {
    match tokens.store {
        Store::Epic => {
            let tokens = valid_tokens(Store::Epic, tokens)?;
            epic::mark_owned(&tokens, games)
        }
        _ => Ok(()),
    }
}

/// Rattache au compte les offres gratuites désignées. Le jeton est rafraîchi
/// au passage : une offre se commande avec un accès valide.
pub fn claim_free(
    tokens: &StoreTokens,
    games: &[FreeGame],
    locale: &str,
) -> Result<Vec<ClaimOutcome>> {
    match tokens.store {
        Store::Epic => {
            let tokens = valid_tokens(Store::Epic, tokens)?;
            epic::claim_free(&tokens, games, locale)
        }
        _ => Err(Error::Unavailable(
            "cette boutique ne publie pas d'offres gratuites".into(),
        )),
    }
}

/// Description, captures, éditeur et date de sortie d'un jeu, lus chez sa
/// boutique. C'est ce que la fiche dépliée montre dans ses encarts centraux.
pub fn details(
    store: Store,
    tokens: &StoreTokens,
    store_id: &str,
    namespace: &str,
    catalog_item: &str,
    locale: &str,
) -> Result<StoreDetails> {
    match store {
        Store::Epic => epic::details(tokens, namespace, catalog_item, locale),
        Store::Gog => gog::details(tokens, store_id, locale),
        Store::Itch => itch::details(tokens, store_id, locale),
        Store::Steam => Err(Error::Unavailable("Steam a sa propre fiche".into())),
    }
}

/// Termine une connexion par code d'autorisation (Epic, GOG).
pub fn login_code(store: Store, code: &str) -> Result<StoreTokens> {
    match store {
        Store::Epic => epic::login_with_code(code),
        Store::Gog => gog::login_with_code(code),
        _ => Err(Error::Unavailable("cette boutique ne se connecte pas par code")),
    }
}

/// Termine une connexion par clé API (itch.io).
pub fn login_key(store: Store, key: &str) -> Result<StoreTokens> {
    match store {
        Store::Itch => itch::login_with_key(key),
        _ => Err(Error::Unavailable("cette boutique ne se connecte pas par clé")),
    }
}

/// Relit un jeton d'accès expiré.
pub fn refresh(store: Store, tokens: &StoreTokens) -> Result<StoreTokens> {
    match store {
        Store::Epic => epic::refresh(tokens),
        Store::Gog => gog::refresh(tokens),
        // Une clé API itch n'expire pas : elle reste valable jusqu'à sa
        // révocation sur le site.
        Store::Itch => Ok(tokens.clone()),
        Store::Steam => Err(Error::Unavailable("Steam a sa propre session")),
    }
}

/// Une seule rotation à la fois : Epic renouvelle aussi le refresh token, et
/// deux lectures simultanées ne doivent pas consommer le même ancien jeton.
static TOKEN_ROTATION: Mutex<()> = Mutex::new(());

/// Jeton d'accès valable, rafraîchi en mémoire si besoin.
///
/// Cette forme ne touche pas au disque : un appelant qui possède le dossier de
/// données doit préférer [`valid_tokens_saved`], sinon le nouveau refresh token
/// rendu par la boutique est perdu.
pub fn valid_tokens(store: Store, tokens: &StoreTokens) -> Result<StoreTokens> {
    let now = lapinux_core::util::now_unix();
    if store == Store::Itch || tokens.fresh(now) {
        return Ok(tokens.clone());
    }
    refresh(store, tokens)
}

/// Jeton d'accès valable, rafraîchi et réécrit sur disque quand il change.
///
/// Le verrou couvre la relecture du fichier : si une autre lecture vient de
/// renouveler les jetons, la version fraîche du disque est reprise au lieu de
/// consommer une seconde fois l'ancien refresh token.
pub fn valid_tokens_saved(
    data_dir: &Path,
    store: Store,
    tokens: &StoreTokens,
) -> Result<StoreTokens> {
    let now = lapinux_core::util::now_unix();
    if store == Store::Itch || tokens.fresh(now) {
        return Ok(tokens.clone());
    }

    let _rotation = TOKEN_ROTATION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let current =
        lapinux_core::stores::load_tokens(data_dir, store).unwrap_or_else(|| tokens.clone());
    let now = lapinux_core::util::now_unix();
    if current.fresh(now) {
        return Ok(current);
    }
    let renewed = refresh(store, &current)?;
    keep_renewed_tokens(data_dir, &current, renewed)
}

/// Enregistre le jeton renouvelé avant qu'il serve : le refresh token qu'Epic
/// vient de rendre ne doit jamais rester uniquement en mémoire.
fn keep_renewed_tokens(
    data_dir: &Path,
    previous: &StoreTokens,
    renewed: StoreTokens,
) -> Result<StoreTokens> {
    if renewed != *previous {
        lapinux_core::stores::save_tokens(data_dir, &renewed).map_err(Error::Io)?;
    }
    Ok(renewed)
}

/// Nom du compte, tel que les réglages l'affichent.
pub fn account_label(store: Store, tokens: &StoreTokens) -> Result<String> {
    if !tokens.account.trim().is_empty() {
        return Ok(tokens.account.clone());
    }
    match store {
        Store::Epic => epic::account_label(tokens),
        Store::Gog => gog::account_label(tokens),
        Store::Itch => itch::account_label(tokens),
        Store::Steam => Err(Error::Unavailable("Steam a sa propre session")),
    }
}

/// Bibliothèque du compte.
pub fn library(store: Store, tokens: &StoreTokens) -> Result<Vec<StoreGame>> {
    let tokens = valid_tokens(store, tokens)?;
    match store {
        Store::Epic => epic::library(&tokens),
        Store::Gog => gog::library(&tokens),
        Store::Itch => itch::library(&tokens),
        Store::Steam => Err(Error::Unavailable("Steam a son propre catalogue")),
    }
}

/// Plan d'installation d'un jeu : ce qu'il faut télécharger et où l'extraire.
///
/// `cdn` préfère un CDN Epic (`cloudfront`, `akamai`, `fastly`) ; vide, Epic
/// choisit, comme son launcher.
pub fn install_plan(
    store: Store,
    tokens: &StoreTokens,
    store_id: &str,
    platform: &str,
    cdn: &str,
) -> Result<InstallPlan> {
    let tokens = valid_tokens(store, tokens)?;
    match store {
        Store::Epic => epic::install_plan(&tokens, store_id, platform, cdn),
        Store::Gog => gog::install_plan(&tokens, store_id, platform),
        Store::Itch => itch::install_plan(&tokens, store_id, platform),
        Store::Steam => Err(Error::Unavailable("Steam a son propre téléchargement")),
    }
}

/// Poids d'un contenu additionnel, quand la boutique en publie un build. Rend
/// aussi son nom d'application, celui qui sert à le télécharger.
pub fn dlc_build(
    store: Store,
    tokens: &StoreTokens,
    namespace: &str,
    catalog_item: &str,
    app_name: &str,
    platform: &str,
) -> Option<(String, u64, u64)> {
    match store {
        Store::Epic => epic::dlc_build(tokens, namespace, catalog_item, app_name, platform),
        _ => None,
    }
}

/// Succès d'un jeu, tels que sa boutique les publie.
///
/// `namespace` est le sandbox Epic, que la bibliothèque du compte nomme ; GOG
/// n'en a pas besoin. itch.io n'expose aucun succès.
pub fn achievements(
    store: Store,
    tokens: &StoreTokens,
    store_id: &str,
    namespace: &str,
) -> Result<Vec<Achievement>> {
    let tokens = valid_tokens(store, tokens)?;
    match store {
        Store::Epic => epic::achievements(&tokens, namespace, "fr"),
        Store::Gog => gog::achievements(&tokens, store_id),
        Store::Itch => Err(Error::Unavailable("itch.io ne publie pas de succès")),
        Store::Steam => Err(Error::Unavailable("Steam a sa propre lecture")),
    }
}

/// Sauvegardes gardées par la boutique pour ce jeu.
///
/// `dir_name` est le nom du dossier de sauvegarde local : GOG range chaque jeu
/// sous un dossier et le rappelle à l'envoi ; Epic n'en tient pas compte.
pub fn cloud_list(
    store: Store,
    tokens: &StoreTokens,
    store_id: &str,
    platform: &str,
    dir_name: &str,
) -> Result<Vec<RemoteFile>> {
    let tokens = valid_tokens(store, tokens)?;
    match store {
        Store::Epic => epic::cloud_list(&tokens, store_id),
        Store::Gog => gog::cloud_list(&tokens, store_id, platform, dir_name),
        Store::Itch => Err(Error::Unavailable("itch.io n'a pas de sauvegardes cloud")),
        Store::Steam => Err(Error::Unavailable("Steam a son propre cloud")),
    }
}

/// Télécharge un fichier de sauvegarde vers `dest`.
pub fn cloud_download(
    store: Store,
    tokens: &StoreTokens,
    store_id: &str,
    platform: &str,
    dir_name: &str,
    file: &RemoteFile,
    dest: &Path,
) -> Result<u64> {
    let tokens = valid_tokens(store, tokens)?;
    match store {
        Store::Epic => epic::cloud_download(&tokens, store_id, file, dest),
        Store::Gog => gog::cloud_download(&tokens, store_id, platform, dir_name, file, dest),
        Store::Itch => Err(Error::Unavailable("itch.io n'a pas de sauvegardes cloud")),
        Store::Steam => Err(Error::Unavailable("Steam a son propre cloud")),
    }
}

/// Envoie un fichier local dans le cloud du jeu, et rend son poids.
pub fn cloud_upload(
    store: Store,
    tokens: &StoreTokens,
    store_id: &str,
    platform: &str,
    dir_name: &str,
    file: &RemoteFile,
    body: Vec<u8>,
    modified: i64,
) -> Result<u64> {
    let tokens = valid_tokens(store, tokens)?;
    match store {
        Store::Epic => epic::cloud_upload(&tokens, store_id, file, body),
        Store::Gog => gog::cloud_upload(&tokens, store_id, platform, dir_name, file, body, modified),
        Store::Itch => Err(Error::Unavailable("itch.io n'a pas de sauvegardes cloud")),
        Store::Steam => Err(Error::Unavailable("Steam a son propre cloud")),
    }
}

/// Exécute un plan : téléchargements puis extraction, dans `dest`.
///
/// `progress` reçoit (octets faits, octets totaux) et rend `false` quand la
/// tâche a été annulée.
pub fn install(
    tokens: &StoreTokens,
    plan: &InstallPlan,
    cache_dir: &Path,
    dest: &Path,
    progress: &mut dyn FnMut(u64, u64) -> bool,
) -> Result<()> {
    match plan.store {
        Store::Epic => epic::install(tokens, plan, cache_dir, dest, progress),
        Store::Gog => gog::install(tokens, plan, cache_dir, dest, progress),
        Store::Itch => itch::install(tokens, plan, cache_dir, dest, progress),
        Store::Steam => Err(Error::Unavailable("Steam a son propre téléchargement")),
    }
}

#[cfg(test)]
mod session_tests {
    use super::*;
    use lapinux_core::stores::{load_tokens, save_tokens};

    fn tokens(access: &str, refresh: &str, expires_at: i64) -> StoreTokens {
        StoreTokens {
            store: Store::Epic,
            account: "Nathounette".into(),
            access_token: access.into(),
            refresh_token: refresh.into(),
            expires_at,
            user_id: "42".into(),
        }
    }

    #[test]
    fn a_renewed_token_is_saved_before_it_is_returned() {
        let dir = tempfile::tempdir().unwrap();
        let previous = tokens("old", "old-refresh", 0);
        let renewed = tokens("new", "new-refresh", 4_102_444_800);
        let kept = keep_renewed_tokens(dir.path(), &previous, renewed.clone()).unwrap();
        assert_eq!(kept, renewed);
        assert_eq!(load_tokens(dir.path(), Store::Epic), Some(renewed));
    }

    #[test]
    fn a_token_renewed_by_another_read_is_reused() {
        let dir = tempfile::tempdir().unwrap();
        let fresh = tokens(
            "new",
            "new-refresh",
            lapinux_core::util::now_unix() + 3_600,
        );
        save_tokens(dir.path(), &fresh).unwrap();
        let stale = tokens("old", "old-refresh", 0);
        let kept = valid_tokens_saved(dir.path(), Store::Epic, &stale).unwrap();
        assert_eq!(kept, fresh);
    }
}
