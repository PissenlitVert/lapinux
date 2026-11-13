//! Protocole IPC (JSON, trames préfixées par leur longueur) sur socket Unix.

use crate::config::Config;
use crate::error::{Error, Result};
use crate::model::{
    CloudSaves, GameSettings, SessionInfo, SteamInputCommunityProfile, WorkshopAction,
};
use crate::social::SteamPresence;
use crate::stores::Store;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const MAX_FRAME: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SteamCloudChoice {
    Local,
    Cloud,
    KeepBoth,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Req {
    Ping,
    Scan,
    Games,
    Game {
        appid: u32,
    },
    /// Lit le moteur, les suréchantillonneurs et l'anti-triche d'un jeu
    /// installé, sans le lancer.
    GameTech {
        appid: u32,
    },
    /// Bibliothèques DLSS du jeu et versions plus récentes disponibles sur la
    /// machine.
    DlssState {
        appid: u32,
    },
    /// Remplace les DLL DLSS du jeu par les plus récentes trouvées.
    DlssUpdate {
        appid: u32,
    },
    /// Remet les DLL DLSS d'origine du jeu.
    DlssRestore {
        appid: u32,
    },
    SaveSettings {
        appid: u32,
        settings: GameSettings,
    },
    Launch {
        appid: u32,
        dry_run: bool,
    },
    Stop,
    Session,
    /// Redonne le focus à la fenêtre du jeu : l'écran d'attente le lui avait
    /// pris, et KWin réactive sinon la fenêtre de Lapinux.
    FocusGame {
        appid: u32,
    },
    /// Parties terminées, de la plus récente à la plus ancienne : la page
    /// Activité en fait des cartes de jeu.
    SessionHistory,
    /// Retire une partie de l'historique : celle qui a commencé à `started`
    /// pour ce jeu. Le temps de jeu cumulé du jeu est diminué d'autant.
    DeleteSession {
        appid: u32,
        started: i64,
    },
    /// Retire toutes les parties d'une journée (`day` = minuit Unix local).
    ClearSessionDay {
        day: i64,
    },
    Jobs,
    /// Installe un jeu ; `dlc` liste les contenus additionnels possédés à
    /// télécharger avec lui, leurs dépôts rejoignant ceux de jeu de base.
    /// `platform` choisit la version publiée (`linux`, `windows`, `macos`) ;
    /// sans elle, le moteur applique sa préférence.
    InstallGame {
        appid: u32,
        #[serde(default)]
        dlc: Vec<u32>,
        #[serde(default)]
        platform: Option<String>,
    },
    /// Supprime les fichiers installés d'un jeu, son préfixe et son manifeste.
    UninstallGame {
        appid: u32,
    },
    /// Copie vérifiée puis désinstallation, ou erreur sans suppression.
    UninstallGameWithBackup {
        appid: u32,
    },
    CancelJob {
        id: String,
    },
    /// Met une tâche en pause (ou la reprend) sans perdre sa progression.
    PauseJob {
        id: String,
        paused: bool,
    },
    /// Supprime de l'historique les tâches terminées, échouées ou annulées.
    ClearHistory,
    /// Retire une seule tâche terminée de l'historique, sans toucher aux
    /// téléchargements en cours, en pause ou interrompus.
    DeleteJob {
        id: String,
    },
    /// Réordonne les tâches : les identifiants fournis passent devant, dans
    /// cet ordre. Les tâches absentes de la liste gardent leur ordre relatif.
    ReorderJobs {
        ids: Vec<String>,
    },
    Protons,
    ProtonReleases,
    /// Installe une version vérifiée de la couche communautaire afmf-linux.
    InstallFrameGeneration {
        version: String,
    },
    /// Retire seulement les fichiers vérifiés installés par Lapinux.
    RemoveFrameGeneration {
        version: String,
    },
    InstallProton {
        repo: String,
        asset: String,
        checksum_url: Option<String>,
    },
    RemoveProton {
        name: String,
    },
    Doctor,
    OverlayToggle,
    OverlayAction {
        action: String,
    },
    Devices,
    Profiles,
    Config,
    SetConfig {
        config: Config,
    },
    SteamStatus,
    /// Catalogue des jeux possédés ; `refresh` relit les licences Steam.
    SteamCatalog {
        refresh: bool,
    },
    /// Complète le catalogue avec le seul jeu d'une clé activée.
    /// Ajoute au catalogue le jeu d'une clé qui vient d'être activée, sans
    /// relire le compte : la fiche minimale suffit à le montrer.
    SteamCatalogPatch {
        #[serde(default)]
        appid: u32,
        #[serde(default)]
        name: String,
    },
    /// Fiche complète d'un jeu, relue à la demande : le compte par la session,
    /// le protocole par PICS, la boutique par sa page publique.
    GameInfo {
        appid: u32,
    },
    /// Profils Steam Input disponibles pour les manettes branchées.
    SteamInput {
        appid: u32,
    },
    /// Copies de configurations Steam Input conservées dans Lapinux.
    SteamInputSaved,
    /// Sélectionne une copie du profil dans Lapinux pour le lancement direct.
    SteamInputApply {
        appid: u32,
        profile: PathBuf,
    },
    /// Cherche les configurations publiées dans la communauté Steam, y compris
    /// celles qui ne sont pas encore possédées ou téléchargées.
    SteamInputCommunity {
        appid: u32,
        #[serde(default)]
        query: String,
        #[serde(default)]
        controller: Option<String>,
        #[serde(default)]
        page: usize,
    },
    /// Télécharge une configuration communautaire depuis le CDN Steam et
    /// l'applique au jeu, sans passer par l'abonnement Atelier.
    SteamInputCommunityApply {
        appid: u32,
        profile: SteamInputCommunityProfile,
    },
    /// Atelier d'un jeu : nos abonnements, leur poids et le dossier de
    /// rangement `LapinuxLibrary/Mods/<jeu> (<appid>)/SteamWorkshop`.
    SteamWorkshop {
        appid: u32,
        #[serde(default)]
        name: String,
        /// Relit aussi les abonnements sur le site communautaire : un article
        /// suivi depuis la fenêtre web de Lapinux peut manquer au fichier
        /// `userdata` tant que le client Steam ne l'a pas vu.
        #[serde(default)]
        refresh: bool,
    },
    /// Range les abonnements de l'Atelier dans
    /// `LapinuxLibrary/Mods/<jeu> (<appid>)/SteamWorkshop` sous forme de vrais
    /// fichiers : les fichiers de Steam ne bougent pas.
    SteamWorkshopArrange {
        appid: u32,
        #[serde(default)]
        name: String,
    },
    /// Active ou désactive des contenus dans le dossier `Mods`, ou supprime
    /// leurs fichiers locaux. Une liste vide vise tous les contenus installés.
    SteamWorkshopManage {
        appid: u32,
        #[serde(default)]
        name: String,
        #[serde(default)]
        ids: Vec<u64>,
        action: WorkshopAction,
    },
    /// Télécharge des contenus de l'Atelier sans le client Steam, avec
    /// DepotDownloader, puis les range dans `Mods`.
    SteamWorkshopDownload {
        appid: u32,
        #[serde(default)]
        name: String,
        #[serde(default)]
        ids: Vec<u64>,
    },
    /// Configurations connues pour un jeu — correctifs, traductions, mods
    /// déclaratifs — et celles déjà installées.
    GameConfigurations {
        appid: u32,
    },
    /// Installe une configuration sur un jeu : réglages et fichiers, avec
    /// sauvegarde de ce qui est remplacé.
    InstallConfiguration {
        appid: u32,
        id: String,
    },
    /// Retire une configuration et remet le jeu dans son état d'avant.
    RemoveConfiguration {
        appid: u32,
        id: String,
    },
    /// Thèmes proposés par le catalogue et ceux déjà installés.
    ThemeCatalog,
    /// Installe un thème du catalogue : fichier vérifié, alors que le jeu
    /// n'est pas concerné.
    InstallTheme { id: String },
    /// Injecteurs facultatifs suivis par jeu ; les chemins sont relatifs au dossier du jeu.
    GraphicsInjectors {
        appid: u32,
    },
    InstallGraphicsInjector {
        appid: u32,
        kind: String,
        version: String,
        executable: String,
        proxy: String,
    },
    RemoveGraphicsInjector {
        appid: u32,
        kind: String,
    },
    /// Diagnostic calculé sur disque et sur les réglages du jeu.
    GraphicsInjectorStatuses {
        appid: u32,
    },
    /// Repose les fichiers manquants depuis la copie de suivi ; un fichier
    /// modifié est sauvegardé avant restauration, un `ddraw.ini` personnalisé
    /// reste intact.
    RepairGraphicsInjector {
        appid: u32,
        kind: String,
    },
    /// Retire puis réinstalle, uniquement si le retrait est complet.
    ReinstallGraphicsInjector {
        appid: u32,
        kind: String,
    },
    /// Corrige les réglages Lapinux qui bloquent l'override ; les variables
    /// saisies par le joueur ne sont jamais supprimées.
    FixGraphicsInjectorOverride {
        appid: u32,
        kind: String,
    },
    /// Copies `steam_api64.dll.lapinux-orig` laissées par une session
    /// interrompue : le pont Steamworks n'a pas remis le DLL d'origine.
    SteamBridgeStatus {
        appid: u32,
    },
    /// Restaure les DLL d'origine des ponts Steamworks orphelins. Le prochain
    /// lancement reposera le pont proprement.
    RepairSteamBridge {
        appid: u32,
    },
    /// Succès d'un jeu lus par la session Steam enregistrée : le schéma du jeu
    /// et l'état du compte.
    GameAchievements {
        appid: u32,
    },
    /// Affiche le bandeau d'un succès, comme s'il venait d'être débloqué.
    /// `api_name` choisit le succès (bouton de test de la liste complète) ;
    /// sans lui, le daemon prend le dernier débloqué du jeu.
    AchievementToast {
        appid: u32,
        api_name: String,
    },
    /// Débloque des succès Steam au nom du jeu, par la session partagée du
    /// daemon. Le pont Steamworks s'en sert : une seule connexion au CM pour
    /// tout Lapinux, sinon la seconde ferme la première.
    StoreAchievements {
        appid: u32,
        names: Vec<String>,
    },
    /// Sauvegardes Steam Cloud d'un jeu, lues par la session enregistrée : la
    /// liste des fichiers gardés, le quota consommé et ce que la dernière
    /// partie y a changé.
    SteamCloud {
        appid: u32,
    },
    /// Lecture fraîche, notamment après une divergence ou une erreur.
    SteamCloudFresh {
        appid: u32,
    },
    /// Récupère du cloud les sauvegardes d'un jeu et les écrit dans son
    /// préfixe Proton, là où le jeu les lit.
    CloudRestore {
        appid: u32,
    },
    /// Envoie vers le cloud les sauvegardes locales d'un jeu, depuis ses
    /// réglages ; la sortie de session le fait d'elle-même.
    CloudUpload {
        appid: u32,
    },
    /// Résolution explicite après présentation des deux états au joueur.
    ResolveSteamCloud {
        appid: u32,
        choice: SteamCloudChoice,
        expected: CloudSaves,
    },
    /// Résolution explicite d'une divergence Epic ou GOG.
    ResolveStoreCloud {
        appid: u32,
        choice: SteamCloudChoice,
        expected: CloudSaves,
    },
    /// Copies locales datées des sauvegardes d'un jeu, de la plus récente à la
    /// plus ancienne.
    SaveSnapshots {
        appid: u32,
    },
    /// Journal des sauvegardes d'un jeu : copies, restaurations et exports,
    /// du plus récent au plus ancien.
    SaveHistory {
        appid: u32,
    },
    /// Prend une copie locale des sauvegardes d'un jeu, sans attendre la fin
    /// d'une partie. `slot` la range dans un emplacement précis (1 à 3) ; sans
    /// lui, la première place libre est utilisée, sinon la plus ancienne.
    BackupSaves {
        appid: u32,
        #[serde(default)]
        slot: Option<u8>,
    },
    /// Remet une copie locale à la place des sauvegardes du jeu.
    RestoreSaves {
        appid: u32,
        snapshot: String,
    },
    /// Recopie toutes les copies locales d'un jeu vers un dossier choisi.
    ExportSaves {
        appid: u32,
        dest: String,
    },
    /// Retire une copie locale précise des emplacements du jeu.
    DeleteSaveSnapshot {
        appid: u32,
        snapshot: String,
    },
    /// Poids et nombre de copies locales de tous les jeux, pour les réglages.
    SavesOverview,
    /// Enregistrement vidéo de la partie en cours : `start` le lance, sinon
    /// l'arrête et garde le fichier. Seule la fenêtre du jeu est capturée.
    Recording {
        start: bool,
    },
    /// Suspend ou reprend l'enregistrement en cours, sans le terminer. Une
    /// pause manuelle reste acquise après la fermeture du menu rapide.
    RecordingPause {
        paused: bool,
    },
    /// État de l'enregistrement : pause en cours et secondes déjà captées.
    /// L'overlay l'interroge chaque seconde pour son bandeau.
    RecordingStatus,
    /// Volume du flux audio du jeu : sans `volume`, le daemon le relit ;
    /// avec, il l'applique au flux et confirme la valeur effective.
    GameVolume {
        volume: Option<f32>,
    },
    /// Charge du processeur, de la mémoire et de la carte graphique, relevée
    /// pour le HUD de performance de l'overlay.
    System,
    /// Démarre une connexion Lapinux en arrière-plan ; rendre la main tout de
    /// suite permet d'attendre un code Steam Guard sans bloquer l'interface.
    /// `token` est un jeton de refresh Lapinux collé dans l'écran de connexion :
    /// il remplace le mot de passe et sert de repli à la session enregistrée.
    SteamLogin {
        username: String,
        #[serde(default)]
        password: String,
        #[serde(default)]
        guard: Option<String>,
        #[serde(default)]
        token: Option<String>,
    },
    /// Connexion par QR code : l'application Steam approuve le défi affiché.
    SteamLoginQr,
    /// Répond au code Steam Guard demandé par une connexion en cours.
    SteamLoginGuard {
        code: String,
    },
    /// Annule la connexion Steam en cours et libère l'écran de connexion.
    SteamLoginCancel,
    /// Supprime le jeton Lapinux enregistré.
    SteamForgetToken,
    /// Déconnecte le compte Steam : jeton, identité et session sociale.
    SteamDisconnect,
    /// Vérifie auprès de Steam que le jeton enregistré ouvre encore une
    /// session ; le verdict revient dans `SteamStatus.token`.
    SteamTokenCheck,
    /// État social Steam : présence publiée, amis, notifications et famille.
    /// `refresh` force une relecture si la session dort.
    SteamSocial {
        #[serde(default)]
        refresh: bool,
    },
    /// Change la présence publiée sur Steam pendant la session sociale.
    SteamSocialPresence {
        presence: SteamPresence,
    },
    /// Liste d'amis, avatars et discussions de l'overlay.
    SteamFriendsState,
    /// Active une clé produit Steam sur le compte connecté ; le verdict
    /// revient dans `SteamSocial::redeem`.
    SteamSocialRedeemKey {
        key: String,
    },
    /// Envoie un message Steam à un ami.
    SteamSocialMessage {
        steam_id: u64,
        text: String,
    },
    /// Envoie un autocollant Steam à un ami.
    SteamSocialSticker {
        steam_id: u64,
        name: String,
    },
    /// Demande l'historique récent d'une conversation Steam. `older` demande
    /// la page précédente quand Steam a signalé d'autres messages. Les
    /// messages rejoignent l'instantané social relu par l'interface.
    SteamChatHistory {
        steam_id: u64,
        #[serde(default)]
        older: bool,
    },
    /// Importe un export JSON de conversation dans l'archive locale. Les
    /// messages antérieurs à Lapinux complètent ainsi l'historique que Steam
    /// ne renvoie plus.
    SteamChatImport {
        steam_id: u64,
        path: String,
    },
    /// Crée un lobby Steam pour un jeu et rend son `steam_id_lobby`.
    SteamLobbyCreate {
        appid: u32,
        max_members: u32,
        /// Type de lobby : 0 privé, 1 amis, 2 public (défaut).
        #[serde(default)]
        lobby_type: Option<i32>,
        /// Drapeaux de lobby (défaut 1).
        #[serde(default)]
        lobby_flags: Option<i32>,
        /// Cellule CM à annoncer ; sans valeur, celle de la session.
        #[serde(default)]
        cell_id: Option<u32>,
    },
    /// Invite un ami dans le dernier lobby Steam créé.
    SteamLobbyInvite {
        steam_id: u64,
    },
    /// Rejoint le lobby d'un ami : `connect` porte l'adresse `ip:port` de la
    /// partie, telle qu'enregistrée à la création du lobby.
    SteamLobbyJoin {
        appid: u32,
        lobby: u64,
        connect: String,
    },
    /// Journaux de l'application : liste des fichiers et dernières lignes.
    /// `file` est un nom présent dans le dossier des journaux, jamais un chemin.
    Logs {
        #[serde(default)]
        file: Option<String>,
        #[serde(default)]
        lines: usize,
    },
    Stats,
    /// Comptes et bibliothèques des trois boutiques (Epic, GOG, itch.io) ;
    /// `refresh` relit les bibliothèques au lieu de rendre le cache.
    Stores {
        #[serde(default)]
        refresh: bool,
        /// Boutiques à relire ; vide, toutes les boutiques connectées.
        #[serde(default)]
        only: Vec<Store>,
    },
    /// Enregistre un compte boutique à partir de son jeton (ou de la clé
    /// d'API pour itch.io) et relit sa bibliothèque.
    StoreLogin {
        store: Store,
        #[serde(default)]
        token: String,
        #[serde(default)]
        username: String,
    },
    /// Déconnecte une boutique : le jeton est supprimé du disque.
    StoreLogout {
        store: Store,
    },
    /// Démarre le téléchargement d'un jeu d'une boutique dans la bibliothèque
    /// Lapinux. `platform` choisit le condensat publié (`linux`, `windows`,
    /// `macos`) ; sans lui, la boutique applique sa préférence.
    StoreInstall {
        store: Store,
        store_id: String,
        #[serde(default)]
        platform: Option<String>,
    },
    /// Description, captures et éditeur d'un jeu de boutique, lus chez elle
    /// quand la fiche se déplie.
    StoreDetails {
        appid: u32,
    },
    /// Poids publié d'un jeu de boutique : ce que le téléchargement demande et
    /// ce que le jeu occupe une fois installé.
    StoreSize {
        appid: u32,
    },
    /// Poids de chaque contenu additionnel d'un jeu de boutique : ce que son
    /// téléchargement demande et ce qu'il occupe une fois installé.
    StoreDlcSizes {
        appid: u32,
    },
    /// Jeux de boutique installés dont la boutique a publié un autre build.
    StoreUpdates,
    /// Jeux gratuits en cours et à venir chez les boutiques : la page
    /// « Jeux gratuits » de la section Boutiques les liste avec leur échéance.
    StoreFreeGames,
    /// Rattache au compte les offres gratuites désignées par leur identifiant
    /// `<espace de noms>:<offre>`.
    StoreClaimFreeGames {
        ids: Vec<String>,
    },
    /// Amis des comptes de boutique : Epic Games et GOG publient leur liste,
    /// itch.io n'en a pas. Chaque boutique répond pour elle, une erreur chez
    /// l'une ne devant pas effacer les autres.
    StoreFriends,
    /// Notifications personnelles des boutiques : demandes d'ami Epic et
    /// invitations GOG. itch.io n'en publie aucune dans son API.
    StoreNotifications,
    /// Ce que l'interface affiche : la section ouverte et le jeu dont la fiche
    /// s'affiche. Discord Rich Presence s'en sert pour nommer l'activité.
    DiscordView {
        #[serde(default)]
        section: String,
        #[serde(default)]
        game: Option<String>,
        /// Ligne chiffrée de la fiche ouverte : boutique, temps de jeu et
        /// installation.
        #[serde(default)]
        note: Option<String>,
        /// Jaquette du jeu dont la fiche est ouverte : la grande vignette de
        /// l'activité.
        #[serde(default)]
        cover: Option<String>,
        /// Boutique du jeu dont la fiche est ouverte, ou de la page
        /// Boutiques : elle nomme la page et porte sa vignette.
        #[serde(default)]
        store: Option<Store>,
        /// Identifiant du jeu dans sa boutique : avec la boutique, le daemon
        /// retrouve son application Discord.
        #[serde(default)]
        store_id: Option<String>,
        /// Appid du jeu Steam dont la fiche est ouverte : le daemon y
        /// retrouve l'application Discord officielle, qui donne le titre.
        #[serde(default)]
        appid: Option<u32>,
    },
    /// Active ou coupe la Rich Presence : l'interface la coupe en se fermant,
    /// pour qu'aucune activité ne reste affichée après l'application.
    DiscordPresence {
        active: bool,
    },
    /// Vide les caches de Lapinux : jaquettes, fiches, visuels et pages de
    /// boutique. La configuration, les journaux et les jeux ne sont pas touchés.
    ClearCaches,
    /// Dernière release GitHub du projet, comparée au binaire courant.
    UpdateCheck,
    /// Télécharge l'AppImage d'une release, en arrière-plan.
    UpdateDownload {
        tag: String,
        name: String,
        url: String,
        size: u64,
        sha256: Option<String>,
    },
    /// Progression du téléchargement de mise à jour, s'il y en a un.
    UpdateState,
    /// Remplace l'AppImage courante par celle qui a été téléchargée.
    UpdateInstall,
    Shutdown,
    /// Atomiquement : refuse si le daemon traite une requête ou possède une
    /// tâche non terminée. Les anciennes versions ne connaissent pas ce message.
    ShutdownIfIdle,
}

/// Empreinte du binaire du daemon, renvoyée par `Ping`.
///
/// L'interface compare cette date à celle de son propre binaire : un daemon
/// lancé avant une mise à jour ignore les réglages récents et les efface en
/// réenregistrant la configuration, il doit donc être remplacé.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonBuild {
    /// Date de modification du binaire au démarrage du daemon, en
    /// millisecondes Unix.
    pub stamp: u64,
    /// Version compilée dans ce binaire.
    #[serde(default)]
    pub version: String,
    /// Démarrage du daemon, en secondes Unix.
    #[serde(default)]
    pub started: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Envelope {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub data: serde_json::Value,
}

impl Envelope {
    pub fn ok<T: Serialize>(value: &T) -> Self {
        Self {
            ok: true,
            error: None,
            data: serde_json::to_value(value).unwrap_or(serde_json::Value::Null),
        }
    }

    pub fn unit() -> Self {
        Self {
            ok: true,
            error: None,
            data: serde_json::Value::Null,
        }
    }

    pub fn err(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(message.into()),
            data: serde_json::Value::Null,
        }
    }

    pub fn into_data<T: DeserializeOwned>(self) -> Result<T> {
        if !self.ok {
            return Err(Error::Other(
                self.error.unwrap_or_else(|| "erreur inconnue".into()),
            ));
        }
        serde_json::from_value(self.data).map_err(Error::Json)
    }

    pub fn session_hint(&self) -> Option<SessionInfo> {
        serde_json::from_value(self.data.clone()).ok()
    }
}

pub fn write_frame<W: Write>(w: &mut W, payload: &[u8]) -> std::io::Result<()> {
    if payload.len() > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "trame trop grande",
        ));
    }
    w.write_all(&(payload.len() as u32).to_le_bytes())?;
    w.write_all(payload)?;
    w.flush()
}

pub fn read_frame<R: Read>(r: &mut R) -> std::io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "trame trop grande",
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

pub fn send_request(socket: &Path, req: &Req) -> Result<Envelope> {
    send_request_timeout(socket, req, Duration::from_secs(30))
}

/// Distingue un délai de socket (le daemon écoute mais n'a pas répondu) d'une
/// vraie panne. Sans cela, un daemon occupé passait pour mort et un second
/// processus était lancé par-dessus, incapable de reprendre le socket.
fn ipc_error(error: std::io::Error, timeout: Duration, what: &str) -> Error {
    match error.kind() {
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => Error::IpcTimeout(
            format!("{what} : pas de réponse en {} ms", timeout.as_millis()),
        ),
        _ => Error::Ipc(format!("{what} : {error}")),
    }
}

pub fn send_request_timeout(socket: &Path, req: &Req, timeout: Duration) -> Result<Envelope> {
    let target = socket.display().to_string();
    let mut stream = UnixStream::connect(socket).map_err(|e| ipc_error(e, timeout, &target))?;
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(timeout)).ok();
    let payload = serde_json::to_vec(req)?;
    write_frame(&mut stream, &payload).map_err(|e| ipc_error(e, timeout, "envoi de la requête"))?;
    let reply = read_frame(&mut stream).map_err(|e| ipc_error(e, timeout, "réponse du daemon"))?;
    let env: Envelope = serde_json::from_slice(&reply)?;
    Ok(env)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn frame_roundtrip() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"bonjour").unwrap();
        let mut cursor = Cursor::new(buf);
        assert_eq!(read_frame(&mut cursor).unwrap(), b"bonjour");
    }

    #[test]
    fn oversized_frame_is_rejected() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&(u32::MAX).to_le_bytes());
        let mut cursor = Cursor::new(buf);
        assert!(read_frame(&mut cursor).is_err());
    }

    #[test]
    fn the_install_request_roundtrips() {
        // La version choisie traverse l'IPC : sans elle, le bouton pourrait
        // annoncer Linux et télécharger Windows.
        let req = Req::InstallGame {
            appid: 1017900,
            dlc: vec![900],
            platform: Some("linux".into()),
        };
        let text = serde_json::to_string(&req).unwrap();
        let back: Req = serde_json::from_str(&text).unwrap();
        assert_eq!(back, req);
        // Une interface plus ancienne omet le champ : la demande reste valable.
        let legacy: Req = serde_json::from_str(r#"{"InstallGame":{"appid":1,"dlc":[]}}"#)
            .expect("champ absent accepté");
        assert_eq!(
            legacy,
            Req::InstallGame {
                appid: 1,
                dlc: Vec::new(),
                platform: None
            }
        );
    }

    #[test]
    fn the_configurations_request_roundtrips() {
        // Les configurations d'un jeu se demandent par son identifiant : un
        // champ renommé laisserait la fiche vide, sans autre signe.
        let text = serde_json::to_string(&Req::GameConfigurations { appid: 3311670 }).unwrap();
        let back: Req = serde_json::from_str(&text).unwrap();
        assert!(matches!(back, Req::GameConfigurations { appid: 3311670 }));
        let install = Req::InstallConfiguration {
            appid: 3311670,
            id: "bento-blocks-shared-memory".into(),
        };
        let back: Req = serde_json::from_str(&serde_json::to_string(&install).unwrap()).unwrap();
        assert_eq!(back, install);
    }

    #[test]
    fn the_achievement_request_roundtrips() {
        // L'interface et le daemon ne partagent que ce JSON : un champ renommé
        // ferait échouer la lecture des succès sans autre signe.
        let text = serde_json::to_string(&Req::GameAchievements { appid: 620 }).unwrap();
        let back: Req = serde_json::from_str(&text).unwrap();
        assert!(matches!(back, Req::GameAchievements { appid: 620 }));
    }

    #[test]
    fn the_steam_input_and_workshop_requests_roundtrip() {
        // La fiche et le daemon ne partagent que ce JSON : un champ renommé
        // laisserait les deux nouveaux onglets vides, sans autre signe.
        let input = Req::SteamInput { appid: 620 };
        let back: Req = serde_json::from_str(&serde_json::to_string(&input).unwrap()).unwrap();
        assert_eq!(back, input);
        let saved = Req::SteamInputSaved;
        let back: Req = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        assert_eq!(back, saved);
        let apply = Req::SteamInputApply {
            appid: 620,
            profile: PathBuf::from("/tmp/profil.vdf"),
        };
        let back: Req = serde_json::from_str(&serde_json::to_string(&apply).unwrap()).unwrap();
        assert_eq!(back, apply);
        let community = Req::SteamInputCommunity {
            appid: 620,
            query: "gyro".into(),
            controller: Some("controller_ps5".into()),
            page: 1,
        };
        let back: Req = serde_json::from_str(&serde_json::to_string(&community).unwrap()).unwrap();
        assert_eq!(back, community);
        let profile = SteamInputCommunityProfile {
            id: 791932662,
            appid: 620,
            title: "Gyro".into(),
            controller_type: "controller_ps4".into(),
            download_url: "https://cdn.steamusercontent.com/ugc/1/ABC/".into(),
            ..Default::default()
        };
        let remote_apply = Req::SteamInputCommunityApply {
            appid: 620,
            profile: profile.clone(),
        };
        let back: Req =
            serde_json::from_str(&serde_json::to_string(&remote_apply).unwrap()).unwrap();
        assert_eq!(back, remote_apply);
        let workshop = Req::SteamWorkshop {
            appid: 4000,
            name: "Garry's Mod".into(),
            refresh: true,
        };
        let back: Req = serde_json::from_str(&serde_json::to_string(&workshop).unwrap()).unwrap();
        assert_eq!(back, workshop);
        let arrange = Req::SteamWorkshopArrange {
            appid: 4000,
            name: "Garry's Mod".into(),
        };
        let back: Req = serde_json::from_str(&serde_json::to_string(&arrange).unwrap()).unwrap();
        assert_eq!(back, arrange);
        let manage = Req::SteamWorkshopManage {
            appid: 4000,
            name: "Garry's Mod".into(),
            ids: vec![111, 222],
            action: WorkshopAction::Disable,
        };
        let back: Req = serde_json::from_str(&serde_json::to_string(&manage).unwrap()).unwrap();
        assert_eq!(back, manage);
        let download = Req::SteamWorkshopDownload {
            appid: 4000,
            name: "Garry's Mod".into(),
            ids: vec![111],
        };
        let back: Req = serde_json::from_str(&serde_json::to_string(&download).unwrap()).unwrap();
        assert_eq!(back, download);
    }

    #[test]
    fn envelope_roundtrip() {
        let env = Envelope::ok(&vec![1u32, 2, 3]);
        let text = serde_json::to_string(&env).unwrap();
        let back: Envelope = serde_json::from_str(&text).unwrap();
        assert_eq!(back.into_data::<Vec<u32>>().unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn envelope_error() {
        let env = Envelope::err("boum");
        assert!(env.clone().into_data::<()>().is_err());
        assert_eq!(env.error.as_deref(), Some("boum"));
    }

    /// Un délai de socket est un daemon occupé, pas un daemon mort : le
    /// client ne doit pas le confondre avec une absence de processus.
    #[test]
    fn a_socket_timeout_is_not_a_dead_daemon() {
        use std::io::ErrorKind;
        let timeout = Duration::from_millis(250);
        for kind in [ErrorKind::WouldBlock, ErrorKind::TimedOut] {
            let error = ipc_error(std::io::Error::from(kind), timeout, "réponse du daemon");
            assert!(
                matches!(error, Error::IpcTimeout(ref text) if text.contains("250")),
                "{error}"
            );
        }
        let refused = ipc_error(
            std::io::Error::from(ErrorKind::ConnectionRefused),
            timeout,
            "socket",
        );
        assert!(matches!(refused, Error::Ipc(_)), "{refused}");
    }
}
