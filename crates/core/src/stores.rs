//! Boutiques autres que Steam : identité, capacités, comptes et jetons.
//!
//! Epic, GOG et itch.io n'ont ni le même identifiant de jeu (un `appName`
//! texte, un `product_id` numérique, un `game_id`), ni les mêmes possibilités :
//! itch n'a pas de sauvegardes cloud, aucune des trois n'expose de succès.
//! Ce module porte donc l'identité commune — l'IPC et l'interface n'ont qu'un
//! type à connaître — et le stockage des jetons.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Une boutique de jeux connue de Lapinux.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Store {
    /// Steam : l'unique boutique lue par le protocole du client.
    #[default]
    Steam,
    Epic,
    Gog,
    Itch,
}

impl Store {
    /// Toutes les boutiques, Steam compris.
    pub const ALL: [Store; 4] = [Store::Steam, Store::Epic, Store::Gog, Store::Itch];
    /// Celles qui se connectent hors de Steam.
    pub const EXTERNAL: [Store; 3] = [Store::Epic, Store::Gog, Store::Itch];

    /// Nom affiché.
    pub fn label(self) -> &'static str {
        match self {
            Store::Steam => "Steam",
            Store::Epic => "Epic Games",
            Store::Gog => "GOG",
            Store::Itch => "itch.io",
        }
    }

    /// Adresse de la boutique elle-même : c'est son site que la page
    /// Boutiques ouvre, pas la liste des jeux du compte.
    pub fn site(self) -> &'static str {
        match self {
            Store::Steam => "https://store.steampowered.com/",
            Store::Epic => "https://store.epicgames.com/",
            Store::Gog => "https://www.gog.com/",
            Store::Itch => "https://itch.io/",
        }
    }

    /// Ce que la boutique vend, en une phrase.
    pub fn tagline(self) -> &'static str {
        match self {
            Store::Steam => {
                "La plus grande boutique de jeux PC : soldes, atelier et multijoueur."
            }
            Store::Epic => {
                "Jeux gratuits chaque semaine, exclusivités et Unreal Engine."
            }
            Store::Gog => "Jeux sans DRM, classiques remis au goût du jour et cloud Galaxy.",
            Store::Itch => "Jeux indépendants, prototypes et assets, souvent à prix libre.",
        }
    }

    /// Nom court, sans espace : clé de fichier et de requête.
    pub fn key(self) -> &'static str {
        match self {
            Store::Steam => "steam",
            Store::Epic => "epic",
            Store::Gog => "gog",
            Store::Itch => "itch",
        }
    }

    /// Ce que la boutique sait faire. Les capacités sont des faits de l'API,
    /// pas un réglage : itch n'a pas de cloud, aucune des trois n'a de succès.
    pub fn capabilities(self) -> StoreCapabilities {
        match self {
            Store::Steam => StoreCapabilities {
                library: true,
                install: true,
                cloud: true,
                achievements: true,
            },
            // L'installation Epic passe par le manifeste binaire du build et
            // par les chunks du CDN : le launcher les publie, Legendary et
            // Heroic les lisent, et Lapinux a repris le même chemin.
            //
            // Sauvegardes et succès passent, eux, par `datastorage`
            // (`savesync`) et le GraphQL de la boutique.
            Store::Epic => StoreCapabilities {
                library: true,
                install: true,
                cloud: true,
                achievements: true,
            },
            // GOG s'installe par ses installeurs hors ligne (mojosetup sous
            // Linux, innoextract pour les Windows). Ses sauvegardes passent par
            // `cloudstorage.gog.com` (le même service que Galaxy, avec les
            // identifiants du jeu lus dans son manifeste de build) et ses
            // succès par `gameplay.gog.com`.
            Store::Gog => StoreCapabilities {
                library: true,
                install: true,
                cloud: true,
                achievements: true,
            },
            Store::Itch => StoreCapabilities {
                library: true,
                install: true,
                cloud: false,
                achievements: false,
            },
        }
    }
}

/// Ce qu'une boutique expose, pour que l'interface n'affiche pas un encart
/// vide là où la boutique ne fournit rien.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoreCapabilities {
    pub library: bool,
    pub install: bool,
    pub cloud: bool,
    pub achievements: bool,
}

impl Default for StoreCapabilities {
    fn default() -> Self {
        Self {
            library: false,
            install: false,
            cloud: false,
            achievements: false,
        }
    }
}

/// État d'un compte de boutique, tel que les réglages l'affichent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoreAccount {
    pub store: Store,
    /// Nom du compte tel que la boutique le rend.
    pub account: String,
    pub connected: bool,
    /// Expiration du jeton d'accès, en secondes Unix (0 : pas de jeton).
    pub expires_at: i64,
    /// Ce que la dernière vérification a répondu, ou pourquoi il n'y a rien.
    pub note: String,
    /// Page où l'utilisateur autorise Lapinux, quand la boutique en a une.
    pub login_url: String,
    /// Ce qu'il faut coller après autorisation, dit par la boutique.
    pub login_hint: String,
    pub capabilities: StoreCapabilities,
}

impl Default for StoreAccount {
    fn default() -> Self {
        Self {
            store: Store::Epic,
            account: String::new(),
            connected: false,
            expires_at: 0,
            note: String::new(),
            login_url: String::new(),
            login_hint: String::new(),
            capabilities: StoreCapabilities::default(),
        }
    }
}

impl StoreAccount {
    /// Compte non connecté d'une boutique, avec ses capacités déclarées.
    pub fn disconnected(store: Store) -> Self {
        Self {
            store,
            capabilities: store.capabilities(),
            ..Default::default()
        }
    }
}

/// Un jeu possédé dans une boutique : ce que la bibliothèque affiche avant
/// toute installation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoreGame {
    pub store: Store,
    /// Identifiant de la boutique (appName Epic, product_id GOG, game_id itch).
    pub store_id: String,
    /// Identifiant Lapinux, stable et unique : celui que l'IPC transporte.
    pub appid: u32,
    pub name: String,
    /// Visuel de la fiche, si la boutique en publie un.
    pub cover: String,
    /// Plateformes publiées (`linux`, `windows`, `macos`).
    pub platforms: Vec<String>,
    /// Espace de noms Epic (le « sandbox »), sans lequel les succès et les
    /// sauvegardes ne se nomment pas. Vide pour les autres boutiques.
    #[serde(default)]
    pub namespace: String,
    /// Identifiant de catalogue Epic : la description et les visuels du jeu
    /// se lisent avec lui. Vide pour les autres boutiques.
    #[serde(default)]
    pub catalog_item: String,
    /// Poids publié du build, quand la boutique l'annonce.
    pub size: Option<u64>,
    pub installed: bool,
    /// Outil et non jeu : un asset, un greffon, un modèle ou un moteur du
    /// marketplace Epic. La bibliothèque les cache, les réglages peuvent les
    /// remontrer.
    pub tool: bool,
    /// Éditeurs nommés par la boutique, quand sa liste les publie déjà : les
    /// filtres de la bibliothèque les lisent sans requête supplémentaire.
    #[serde(default)]
    pub publishers: Vec<String>,
    /// Développeurs nommés de la même façon.
    #[serde(default)]
    pub developers: Vec<String>,
    /// Date à laquelle la boutique dit avoir accordé le jeu, en secondes Unix.
    /// Steam et GOG ne la publient pas : elle reste alors inconnue.
    #[serde(default)]
    pub acquired: Option<i64>,
    /// Minutes jouées selon la boutique. Epic les publie, Steam aussi ; GOG et
    /// itch ne donnent rien.
    #[serde(default)]
    pub minutes: Option<u64>,
    /// Contenus additionnels que la boutique rattache à ce jeu et que le
    /// compte possède : Epic les publie comme des fiches à part qui nomment
    /// leur jeu de base.
    #[serde(default)]
    pub dlc: Vec<StoreDlc>,
}

/// Contenu additionnel d'un jeu de boutique : le nom que la boutique lui
/// donne, et l'identifiant qui le distingue chez elle.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoreDlc {
    /// Identifiant de l'offre chez la boutique (l'article de catalogue).
    pub store_id: String,
    pub name: String,
    /// Nom d'application publié par la boutique : c'est lui que le service des
    /// assets reconnaît, donc lui qui porte le build et son poids.
    #[serde(default)]
    pub app_name: String,
    /// Espace de noms de l'offre, demandé avec le nom d'application.
    #[serde(default)]
    pub namespace: String,
}

impl Default for StoreGame {
    fn default() -> Self {
        Self {
            store: Store::Epic,
            store_id: String::new(),
            appid: 0,
            name: String::new(),
            cover: String::new(),
            platforms: Vec::new(),
            namespace: String::new(),
            catalog_item: String::new(),
            size: None,
            installed: false,
            tool: false,
            publishers: Vec::new(),
            developers: Vec::new(),
            acquired: None,
            minutes: None,
            dlc: Vec::new(),
        }
    }
}

/// Ce qu'une boutique publie d'un jeu au-delà de sa jaquette : la description,
/// les captures, l'éditeur et la date de sortie. Lu à la demande, quand la
/// fiche se déplie — la bibliothèque n'a pas à porter mille descriptions.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct StoreDetails {
    pub store: Store,
    /// Description en texte simple : les balises de la boutique sont retirées.
    pub description: String,
    /// Accroche courte de la boutique, distincte de la présentation longue.
    /// Le résumé de la fiche la préfère au début de `description`.
    pub summary: String,
    /// Bande-annonce jouable, quand la boutique ou une fiche Steam du même jeu
    /// en publie une.
    pub trailer: String,
    /// Affiche de la bande-annonce, vide quand elle n'est pas publiée.
    pub trailer_poster: String,
    /// Captures publiées par la boutique, adresses complètes.
    pub screenshots: Vec<String>,
    /// Visuel large de la fiche (bannière), quand la boutique en publie un :
    /// c'est lui qui ouvre la colonne centrale, comme la bannière Steam.
    pub banner: String,
    pub developer: String,
    pub publisher: String,
    /// Date de sortie telle que la boutique l'écrit.
    pub released: String,
    pub genres: Vec<String>,
    /// Page du jeu chez la boutique, quand elle se déduit de son identifiant.
    pub site: String,
    /// Annonces publiées par la boutique pour ce jeu : notes de mise à jour
    /// et actualités, de la plus récente à la plus ancienne.
    pub news: Vec<StoreNews>,
    /// Les annonces viennent de la fiche Steam du même jeu, faute d'annonces
    /// publiées par cette boutique : la fiche le signale près du titre.
    pub news_from_steam: bool,
    /// Le jeu annonce-t-il des sauvegardes cloud ? `None` quand la boutique
    /// ne publie pas cette information pour ce jeu — l'absence n'est pas une
    /// réponse, et la fiche ne doit pas annoncer une synchronisation qui
    /// n'existe pas.
    pub cloud: Option<bool>,
    /// Seconde Unix de la lecture.
    pub fetched: i64,
}

/// Une annonce d'une boutique à propos d'un jeu : le même contenu que les
/// annonces Steam, pour que la fiche les peigne avec le même encart.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoreNews {
    pub title: String,
    /// Début du texte, balises retirées.
    pub excerpt: String,
    /// Texte entier, quand la boutique le publie.
    pub body: String,
    /// Publication, en secondes Unix ; zéro quand la boutique ne la date pas.
    pub date: i64,
    /// Visuel de l'annonce, vide quand la boutique n'en publie pas.
    pub image: String,
    /// Page de l'annonce, vide quand elle n'existe pas.
    pub url: String,
}

impl StoreNews {
    /// L'annonce telle que la fiche Steam la peint : la même structure d'un
    /// encart à l'autre.
    pub fn as_steam(&self) -> crate::model::SteamNews {
        crate::model::SteamNews {
            title: self.title.clone(),
            excerpt: self.excerpt.clone(),
            body: self.body.clone(),
            date: self.date,
            author: String::new(),
            url: self.url.clone(),
            image: self.image.clone(),
        }
    }
}

/// Bibliothèque d'une boutique, telle que le daemon la garde.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoreLibrary {
    pub store: Store,
    pub account: String,
    /// Date de la dernière lecture réussie (secondes Unix).
    pub fetched: i64,
    pub entries: Vec<StoreGame>,
    /// Pourquoi la dernière lecture n'a rien donné.
    pub error: Option<String>,
}

impl Default for StoreLibrary {
    fn default() -> Self {
        Self {
            store: Store::Steam,
            account: String::new(),
            fetched: 0,
            entries: Vec::new(),
            error: None,
        }
    }
}

/// Ce que les réglages affichent : les comptes et les bibliothèques.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoresView {
    pub accounts: Vec<StoreAccount>,
    pub libraries: Vec<StoreLibrary>,
}

/// Clé de comparaison des titres : sans marques déposées ni ponctuation, avec
/// des espaces uniques. Une bibliothèque ne montre qu'une ligne pour
/// « Shadow Tactics: Blades of the Shogun » et « Shadow Tactics Blades of the
/// Shogun » : c'est le même jeu, vendu deux fois sous deux identifiants.
pub fn title_key(name: &str) -> String {
    let mut key = String::with_capacity(name.len());
    let mut spaced = false;
    for ch in name.chars() {
        if ch.is_alphanumeric() {
            for lower in ch.to_lowercase() {
                key.push(lower);
            }
            spaced = false;
        } else if !spaced && !key.is_empty() {
            key.push(' ');
            spaced = true;
        }
    }
    while key.ends_with(' ') {
        key.pop();
    }
    key
}

/// Identifiant Lapinux d'un jeu de boutique.
///
/// L'IPC, les réglages et la fiche cléent depuis toujours sur un `u32` ; un
/// `appName` Epic ou un `product_id` GOG n'y tient pas. Le jeu reçoit donc un
/// identifiant dérivé de `"<boutique>:<identifiant>"`, stable d'une session à
/// l'autre, et dont le bit haut est réservé : aucun appid Steam réel ne le
/// porte (Steam reste sous 2³¹).
pub fn store_appid(store: Store, store_id: &str) -> u32 {
    // FNV-1a 64, ramené sur 31 bits utiles.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in store.key().bytes().chain(b":".iter().copied()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    for byte in store_id.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    (hash as u32 & 0x7fff_ffff) | 0x8000_0000
}

/// Jetons d'une boutique, gardés sur disque.
///
/// Le fichier ne contient que ce que la boutique a rendu : jetons et date
/// d'expiration. Il est écrit en `0600` — lisible par le seul propriétaire —
/// et remplacé d'un bloc, pour qu'une coupure ne laisse pas un demi-jeton.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoreTokens {
    pub store: Store,
    pub account: String,
    pub access_token: String,
    pub refresh_token: String,
    /// Expiration de l'accès (secondes Unix).
    pub expires_at: i64,
    /// Identifiant du compte chez la boutique (Epic `account_id`, GOG `user_id`).
    pub user_id: String,
}

impl StoreTokens {
    pub fn has_refresh(&self) -> bool {
        !self.refresh_token.trim().is_empty()
    }

    /// L'accès est-il encore valable, avec une minute de marge ?
    pub fn fresh(&self, now: i64) -> bool {
        self.expires_at > now + 60
    }
}

/// Un jeu offert par une boutique, avec la fenêtre pendant laquelle il est
/// gratuit. Les offres à venir sont annoncées par la même liste, avec leur
/// date de départ.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FreeGame {
    pub store: Store,
    /// `<espace de noms>:<offre>` : l'identifiant que la boutique attend pour
    /// rattacher l'offre au compte.
    pub id: String,
    pub title: String,
    pub description: String,
    /// Visuel large de l'offre, tel que la boutique le publie.
    pub cover: String,
    /// Page de l'offre chez la boutique.
    pub url: String,
    pub seller: String,
    /// Valeur du jeu hors promotion, déjà formatée par la boutique.
    pub value: String,
    /// Début et fin de l'offre (secondes Unix).
    pub start: i64,
    pub end: i64,
    /// L'offre n'a pas encore commencé : elle appartient à « la semaine
    /// prochaine ».
    pub upcoming: bool,
    /// Plateformes annoncées par la boutique (`pc`, `mobile`).
    pub platforms: Vec<String>,
    /// Le compte possède déjà l'offre : la carte le dit au lieu de proposer
    /// une récupération que la boutique refuserait.
    #[serde(default)]
    pub owned: bool,
    /// L'offre se termine sur la page de la boutique (jeux mobiles) : l'API
    /// du launcher ne la connaît pas, la carte ouvre donc sa page.
    #[serde(default)]
    pub browser_only: bool,
    /// Autres offres du même jeu, une par plateforme : Android et iOS ont
    /// chacun la leur, avec un identifiant différent. Une seule carte les
    /// présente, mais le panier doit toutes les recevoir.
    #[serde(default)]
    pub offers: Vec<String>,
}

/// Ce qu'a donné une récupération, jeu par jeu : la boutique peut refuser une
/// offre (déjà possédée, région, vérification) sans que les autres échouent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClaimOutcome {
    pub id: String,
    pub title: String,
    pub ok: bool,
    #[serde(default)]
    pub error: String,
    /// Epic renvoie vers son passage en caisse au lieu de créer le droit :
    /// l'offre se termine sur la page de la boutique, en un clic.
    #[serde(default)]
    pub needs_checkout: bool,
}

/// Fichier d'identité laissé dans un jeu installé par Lapinux.
///
/// Un dossier sous `steamapps/common` ne dit pas d'où il vient : sans cette
/// marque, un jeu Epic deviendrait un jeu Steam au premier scan de la
/// bibliothèque, et sa fiche chercherait un appid Steam qui n'existe pas.
pub const MARK_FILE: &str = ".lapinux-store.json";

/// Identité d'un jeu installé : sa boutique, son identifiant chez elle et le
/// condensat retenu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoreMark {
    pub store: Store,
    pub store_id: String,
    /// Version installée (`linux`, `windows`, `macos`).
    pub platform: String,
    pub installdir: String,
    pub name: String,
    /// Empreinte du build installé. Deux builds différents n'ont pas les
    /// mêmes, c'est ce qui signale une mise à jour publiée par la boutique.
    /// Vide pour une installation antérieure.
    #[serde(default)]
    pub build: String,
    /// Version de la méthode d'empreinte. Zéro pour les marques écrites avant
    /// l'identité stable d'Epic : leur empreinte d'adresse CDN ne peut pas être
    /// comparée, elle changeait à chaque lecture.
    #[serde(default)]
    pub fingerprint_version: u8,
}

impl Default for StoreMark {
    fn default() -> Self {
        Self {
            store: Store::Steam,
            store_id: String::new(),
            platform: String::new(),
            installdir: String::new(),
            name: String::new(),
            build: String::new(),
            fingerprint_version: 0,
        }
    }
}

impl StoreMark {
    /// Identité stable d'un build Epic : version et empreintes des chunks.
    pub const FINGERPRINT_VERSION: u8 = 2;

    /// Lit la marque d'un dossier installé ; `None` pour un jeu Steam, qui
    /// n'en porte pas.
    pub fn read(install_dir: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(install_dir.join(MARK_FILE)).ok()?;
        let mark: StoreMark = serde_json::from_str(&text).ok()?;
        (mark.store != Store::Steam && !mark.store_id.trim().is_empty()).then_some(mark)
    }

    /// Écrit la marque dans le dossier installé.
    pub fn write(&self, install_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(install_dir)?;
        let text = serde_json::to_string_pretty(self).unwrap_or_default();
        std::fs::write(install_dir.join(MARK_FILE), text)
    }
}

/// Dossier des jetons de boutiques.
pub fn stores_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("stores")
}

/// Fichier des jetons d'une boutique.
pub fn tokens_path(data_dir: &Path, store: Store) -> PathBuf {
    stores_dir(data_dir).join(format!("{}.json", store.key()))
}

/// Lit les jetons enregistrés, ou `None` s'il n'y en a pas.
pub fn load_tokens(data_dir: &Path, store: Store) -> Option<StoreTokens> {
    let text = std::fs::read_to_string(tokens_path(data_dir, store)).ok()?;
    let mut tokens: StoreTokens = serde_json::from_str(&text).ok()?;
    tokens.store = store;
    Some(tokens)
}

/// Écrit les jetons : dossier créé au besoin, fichier en `0600`, remplacement
/// atomique.
pub fn save_tokens(data_dir: &Path, tokens: &StoreTokens) -> std::io::Result<()> {
    use std::io::Write;
    let dir = stores_dir(data_dir);
    std::fs::create_dir_all(&dir)?;
    let path = tokens_path(data_dir, tokens.store);
    let temp = dir.join(format!(".{}.json.tmp", tokens.store.key()));
    let text = serde_json::to_string_pretty(tokens).unwrap_or_default();
    {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(text.as_bytes())?;
        file.sync_all().ok();
    }
    std::fs::rename(&temp, &path)?;
    Ok(())
}

/// Efface les jetons enregistrés d'une boutique.
pub fn clear_tokens(data_dir: &Path, store: Store) -> std::io::Result<()> {
    match std::fs::remove_file(tokens_path(data_dir, store)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_store_game_gets_a_stable_identifier() {
        let first = store_appid(Store::Epic, "Sugar");
        // Même entrée, même identifiant — d'une session à l'autre.
        assert_eq!(first, store_appid(Store::Epic, "Sugar"));
        // Le bit haut est réservé : aucun appid Steam ne peut le porter.
        assert!(first & 0x8000_0000 != 0);
        // Deux boutiques ne se rencontrent pas sur le même identifiant.
        assert_ne!(first, store_appid(Store::Gog, "Sugar"));
        assert_ne!(
            store_appid(Store::Gog, "1207658691"),
            store_appid(Store::Gog, "1207658692")
        );
    }

    #[test]
    fn capabilities_tell_what_each_store_really_offers() {
        // Toutes annoncent leur bibliothèque.
        for store in Store::EXTERNAL {
            assert!(store.capabilities().library);
        }
        // Epic et GOG ont cloud et succès ; itch n'a ni l'un ni l'autre.
        assert!(!Store::Itch.capabilities().cloud);
        assert!(!Store::Itch.capabilities().achievements);
        assert!(Store::Epic.capabilities().cloud);
        assert!(Store::Epic.capabilities().achievements);
        assert!(Store::Gog.capabilities().cloud);
        assert!(Store::Gog.capabilities().achievements);
        // Epic s'installe par son manifeste de build et les chunks du CDN.
        assert!(Store::Epic.capabilities().install);
        assert!(Store::Gog.capabilities().install);
        assert!(Store::Itch.capabilities().install);
        assert!(Store::Steam.capabilities().achievements);
    }

    #[test]
    fn tokens_are_written_for_the_owner_only() {
        let root = std::env::temp_dir().join(format!(
            "lapinux-tokens-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let tokens = StoreTokens {
            store: Store::Gog,
            account: "Nathounette".into(),
            access_token: "access".into(),
            refresh_token: "refresh".into(),
            expires_at: 4_102_444_800,
            user_id: "42".into(),
        };
        save_tokens(&root, &tokens).expect("écriture");
        let path = tokens_path(&root, Store::Gog);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "jetons lisibles par tous");
        }
        assert_eq!(load_tokens(&root, Store::Gog), Some(tokens.clone()));
        // Une autre boutique n'a rien.
        assert!(load_tokens(&root, Store::Epic).is_none());
        clear_tokens(&root, Store::Gog).expect("effacement");
        assert!(load_tokens(&root, Store::Gog).is_none());
        // Effacer deux fois ne fait pas d'erreur.
        clear_tokens(&root, Store::Gog).expect("déjà effacé");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_store_game_keeps_its_identity_through_a_rescan() {
        let root = std::env::temp_dir().join(format!(
            "lapinux-mark-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let mark = StoreMark {
            store: Store::Gog,
            store_id: "1207658691".into(),
            platform: "windows".into(),
            installdir: "The Witcher 3".into(),
            name: "The Witcher 3".into(),
            build: "0123456789abcdef".into(),
            fingerprint_version: StoreMark::FINGERPRINT_VERSION,
        };
        assert!(StoreMark::read(&root).is_none(), "dossier pas encore installé");
        mark.write(&root).expect("marque");
        assert_eq!(StoreMark::read(&root), Some(mark));
        // Un dossier Steam n'a pas de marque : il reste un jeu Steam.
        let steam = StoreMark {
            store: Store::Steam,
            ..Default::default()
        };
        steam.write(&root).expect("marque");
        assert!(StoreMark::read(&root).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn titles_differing_by_punctuation_share_a_key() {
        assert_eq!(
            title_key("Shadow Tactics: Blades of the Shogun"),
            title_key("Shadow Tactics Blades of the Shogun")
        );
        assert_eq!(title_key("Assassin’s Creed® Valhalla"), "assassin s creed valhalla");
        // Deux jeux distincts gardent deux clés.
        assert_ne!(title_key("GTA V"), title_key("GTA V Enhanced"));
        assert_eq!(title_key("  "), "");
    }

    #[test]
    fn an_access_token_keeps_a_minute_of_margin() {
        let tokens = StoreTokens {
            expires_at: 1_000,
            ..Default::default()
        };
        assert!(tokens.fresh(900));
        assert!(!tokens.fresh(950));
        assert!(!tokens.fresh(1_000));
    }
}

/// Date publiée par une boutique, en secondes Unix.
///
/// Epic écrit `2020-05-14T13:22:11.123Z`, itch `2020-05-14 13:22:11` : les
/// deux se lisent sans dépendance horaire, en temps universel.
pub fn parse_stamp(text: &str) -> Option<i64> {
    let text = text.trim();
    if text.len() < 19 {
        return None;
    }
    let number = |from: usize, len: usize| -> Option<i64> { text.get(from..from + len)?.parse().ok() };
    let (year, month, day) = (number(0, 4)?, number(5, 2)?, number(8, 2)?);
    let (hour, minute, second) = (number(11, 2)?, number(14, 2)?, number(17, 2)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Jours depuis l'époque, par l'algorithme civil de Howard Hinnant.
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let doy = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    (days >= 0).then_some(days * 86_400 + hour * 3600 + minute * 60 + second)
}

#[cfg(test)]
mod stamp_tests {
    use super::parse_stamp;

    /// Epic écrit `2020-05-14T13:22:11.123Z`, itch `2020-05-14 13:22:11` :
    /// les deux tombent sur la même seconde.
    #[test]
    fn store_dates_become_unix_seconds() {
        assert_eq!(parse_stamp("2020-05-14T13:22:11.123Z"), Some(1_589_462_531));
        assert_eq!(parse_stamp("2020-05-14 13:22:11"), Some(1_589_462_531));
        assert_eq!(parse_stamp("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_stamp("n/a"), None);
        assert_eq!(parse_stamp(""), None);
    }
}
