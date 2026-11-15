//! Catalogue des jeux possédés : cache disque des licences Steam.
//!
//! La session CM Lapinux rend les paquets ; PICS les étend en appids et fournit
//! les noms. Le catalogue est réécrit à chaque synchronisation.

use crate::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

/// Version des fiches écrites dans le cache. Un catalogue plus ancien est
/// resynchronisé une fois pour recevoir les champs ajoutés depuis.
///
/// Version 16 : les jeux prêtés par la famille reçoivent leur date
/// d'obtention (`rt_time_acquired`), au lieu de rester sans date.
///
/// Version 15 : les jeux sans fiche PICS reçoivent la liste de leurs DLC
/// publiés par la boutique, pour que les versions familiales montrent aussi
/// leurs contenus.
///
/// Version 14 : les DLC partagés viennent des licences du groupe, qui nomment
/// le membre propriétaire de chaque paquet, et non du service de famille —
/// celui-ci ne publie jamais de contenu additionnel.
///
/// Version 13 : les DLC partagés par la famille gardent leur propriétaire.
/// Le type 4 de Steam (outils, SDK) n'est pas un DLC : la version 13 écarte
/// le cache de la version 12 qui les avait pris pour tels.
pub const VERSION: u32 = 16;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub appid: u32,
    #[serde(default)]
    pub name: String,
    /// `Game`, `DLC`, `Tool`, `Demo`… vide tant que Steam n'a pas répondu.
    #[serde(default)]
    pub kind: String,
    /// Jeu prêté par un autre membre de la famille Steam.
    #[serde(default)]
    pub family: bool,
    /// Fiche Steam lue dans PICS (genres, éditeur, dates, images…).
    #[serde(default)]
    pub details: lapinux_core::model::SteamDetails,
    /// Date d'ajout de la licence du jeu au compte, en secondes Unix. Steam la
    /// publie dans sa liste de licences, pas dans ses fiches.
    #[serde(default)]
    pub acquired: Option<i64>,
    /// Minutes jouées telles que Steam les compte, tous systèmes confondus.
    /// `Player.GetOwnedGames` les publie pour toute la bibliothèque.
    #[serde(default)]
    pub minutes: Option<u64>,
}

impl Entry {
    /// Un jeu affichable : un DLC ou un outil n'a pas sa place dans la grille,
    /// pas plus qu'un appid dont Steam n'a rien voulu dire.
    pub fn is_game(&self) -> bool {
        self.kind.eq_ignore_ascii_case("game")
    }

    /// Une démo Steam : une application distincte, jouable, que la
    /// bibliothèque peut afficher sur demande.
    pub fn is_demo(&self) -> bool {
        self.kind.eq_ignore_ascii_case("demo")
    }

    pub fn display_name(&self) -> String {
        if self.name.is_empty() {
            format!("App {}", self.appid)
        } else {
            self.name.clone()
        }
    }
}

/// Contenu additionnel possédé : de quoi l'afficher dans la fiche du jeu et
/// calculer le poids d'un téléchargement qui l'inclut. Les DLC publiés mais
/// non possédés n'y figurent pas : ils ne se téléchargent pas.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DlcEntry {
    pub appid: u32,
    pub name: String,
    /// Octets une fois installé.
    pub install: u64,
    /// Octets à télécharger.
    pub download: u64,
}

/// Contenu additionnel partagé par la famille : son appid, son nom, et les
/// membres qui le possèdent. La même application peut appartenir à plusieurs
/// membres ; elle est alors proposée pour chacun d'eux.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FamilyDlc {
    pub appid: u32,
    pub name: String,
    #[serde(default)]
    pub owners: Vec<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Catalog {
    /// Version des fiches ; `0` pour un cache écrit avant le versionnement.
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub account: String,
    #[serde(default)]
    pub updated: i64,
    #[serde(default)]
    pub entries: Vec<Entry>,
    /// Contenus additionnels possédés, tous jeux confondus.
    #[serde(default)]
    pub dlcs: Vec<DlcEntry>,
    /// Contenus additionnels partagés par la famille et leur propriétaire.
    #[serde(default)]
    pub family_dlcs: Vec<FamilyDlc>,
    /// Dates posées à l'activation d'une clé. Elles priment sur celles qu'une
    /// relève devine par les licences et les lots, souvent plus anciennes et
    /// fausses pour un jeu reçu par une clé.
    #[serde(default)]
    pub stamped: HashMap<u32, i64>,
}

impl Catalog {
    /// Le cache contient-il les fiches complètes attendues par cette version ?
    pub fn is_current(&self) -> bool {
        self.version >= VERSION
    }

    pub fn load(path: &Path) -> Self {
        let mut catalog: Self = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        catalog.fill_missing_art();
        catalog
    }

    /// Les illustrations d'un jeu se déduisent de son appid : une fiche qui
    /// n'en porte pas — écrite par un chemin qui ne les remplissait pas — se
    /// complète ici plutôt que de rester sans bannière, ni logo, ni icône.
    fn fill_missing_art(&mut self) {
        const CDN: &str = "https://cdn.cloudflare.steamstatic.com/steam/apps";
        for entry in &mut self.entries {
            let appid = entry.appid;
            if appid == 0 {
                continue;
            }
            let details = &mut entry.details;
            if details.header_image.is_empty() {
                details.header_image = format!("{CDN}/{appid}/header.jpg");
            }
            if details.library_image.is_empty() {
                details.library_image = format!("{CDN}/{appid}/library_600x900.jpg");
            }
            if details.hero_image.is_empty() {
                details.hero_image = format!("{CDN}/{appid}/library_hero.jpg");
            }
            if details.logo_image.is_empty() {
                details.logo_image = format!("{CDN}/{appid}/logo.png");
            }
            if details.capsule_image.is_empty() {
                details.capsule_image = format!("{CDN}/{appid}/capsule_231x87.jpg");
            }
        }
    }

    /// Écrit à côté puis renomme : une synchronisation en cours ne doit jamais
    /// laisser un catalogue tronqué à l'interface qui le lit.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    }

    pub fn get(&self, appid: u32) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.appid == appid)
    }

    /// Nom et type d'une application, lus sur la boutique publique. Sert au
    /// contrôle léger du compte : un appid ajouté depuis la dernière
    /// synchronisation se nomme sans relire les paquets. Le type évite de
    /// prendre un contenu additionnel ou un outil pour un jeu.
    pub fn store_entry(appid: u32) -> Option<(String, String)> {
        static ENTRIES: OnceLock<Mutex<HashMap<u32, (String, String)>>> = OnceLock::new();
        let entries = ENTRIES.get_or_init(|| Mutex::new(HashMap::new()));
        if let Some(known) = entries
            .lock()
            .ok()
            .and_then(|entries| entries.get(&appid).cloned())
        {
            return Some(known);
        }
        let url = format!(
            "https://store.steampowered.com/api/appdetails?appids={appid}&filters=basic&l=french"
        );
        let found = ureq::get(&url)
            .set("User-Agent", "Lapinux/1.0")
            .timeout(std::time::Duration::from_secs(8))
            .call()
            .ok()
            .and_then(|reply| reply.into_string().ok())
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|value| {
                let data = value.get(appid.to_string())?.get("data")?;
                let name = data.get("name")?.as_str()?.to_string();
                let kind = data
                    .get("type")
                    .and_then(|kind| kind.as_str())
                    .unwrap_or("")
                    .to_string();
                Some((name, kind))
            });
        // Un échec réseau ou une réponse momentanément vide ne doit pas
        // condamner cet appid pour toute la session : le prochain relevé
        // redemandera le nom. Seules les réponses valides sont gardées.
        if let (Some(found), Ok(mut entries)) = (&found, entries.lock()) {
            entries.insert(appid, found.clone());
        }
        found
    }

    /// Nom d'un jeu absent du catalogue — celui d'un ami, par exemple : le
    /// catalogue ne contient que les jeux possédés, alors qu'un ami peut jouer
    /// à n'importe quoi. La boutique publique donne le nom, et il est gardé
    /// pour la suite de la session.
    pub fn store_name(appid: u32) -> Option<String> {
        Self::store_entry(appid).map(|(name, _)| name)
    }

    pub fn dlc(&self, appid: u32) -> Option<&DlcEntry> {
        self.dlcs.iter().find(|entry| entry.appid == appid)
    }

    /// Contenus additionnels **possédés** que ce jeu déclare, dans l'ordre de
    /// ses propres identifiants : la fiche propose ce qu'elle peut installer.
    pub fn dlcs_of(&self, entry: &Entry) -> Vec<DlcEntry> {
        entry
            .details
            .dlc_ids
            .iter()
            .filter_map(|id| self.dlc(*id))
            .cloned()
            .collect()
    }

    /// Ajuste le catalogue à la liste possédée en gardant les noms déjà connus.
    pub fn retain_owned(&mut self, owned: &[u32]) {
        let mut by_appid: HashMap<u32, Entry> = self
            .entries
            .drain(..)
            .map(|entry| (entry.appid, entry))
            .collect();
        self.entries = owned
            .iter()
            .map(|appid| {
                by_appid.remove(appid).unwrap_or(Entry {
                    appid: *appid,
                    ..Default::default()
                })
            })
            .collect();
    }

    /// Appids dont le nom reste à demander à Steam.
    pub fn unnamed(&self, max: usize) -> Vec<u32> {
        self.entries
            .iter()
            .filter(|entry| entry.name.is_empty())
            .map(|entry| entry.appid)
            .take(max)
            .collect()
    }

    pub fn absorb(&mut self, found: Vec<Entry>) {
        for entry in found {
            if let Some(slot) = self
                .entries
                .iter_mut()
                .find(|candidate| candidate.appid == entry.appid)
            {
                // La relève ne sait pas dater un jeu obtenu par une clé : sa
                // fiche revient sans date et effaçait celle qu'on connaissait.
                let mut fresh = entry;
                if fresh.acquired.is_none() {
                    fresh.acquired = slot.acquired;
                }
                if fresh.minutes.is_none() {
                    fresh.minutes = slot.minutes;
                }
                *slot = fresh;
            } else {
                // Le jeu d'une clé activée n'est pas encore au catalogue :
                // « compléter » doit l'ajouter, sinon la relève ciblée lit sa
                // fiche et la jette.
                self.entries.push(entry);
            }
        }
        // Une date posée à l'activation ne se laisse pas remplacer par une
        // date devinée : c'est la seule dont on soit sûr.
        let stamped: Vec<(u32, i64)> = self.stamped.iter().map(|(a, d)| (*a, *d)).collect();
        for (appid, date) in stamped {
            if let Some(slot) = self
                .entries
                .iter_mut()
                .find(|candidate| candidate.appid == appid)
            {
                slot.acquired = Some(date);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(appid: u32, name: &str) -> Entry {
        Entry {
            acquired: None,
            minutes: None,
            appid,
            name: name.to_string(),
            kind: "Game".to_string(),
            family: false,
            details: Default::default(),
        }
    }

    #[test]
    fn retain_keeps_names_and_drops_unowned() {
        let mut catalog = Catalog {
            version: VERSION,
            account: "bob".into(),
            updated: 1,
            entries: vec![entry(440, "Team Fortress 2"), entry(570, "Dota 2")],
            ..Default::default()
        };
        catalog.retain_owned(&[440, 730]);
        assert_eq!(catalog.entries.len(), 2);
        assert_eq!(catalog.get(440).unwrap().name, "Team Fortress 2");
        let fresh = catalog.get(730).unwrap();
        assert!(fresh.name.is_empty());
        assert!(catalog.get(570).is_none());
        assert_eq!(catalog.unnamed(10), vec![730]);
    }

    #[test]
    fn absorb_fills_names_in_place() {
        let mut catalog = Catalog {
            entries: vec![
                Entry {
                    acquired: None,
                    minutes: None,
                    appid: 730,
                    ..Default::default()
                },
                entry(440, "Team Fortress 2"),
            ],
            ..Default::default()
        };
        catalog.absorb(vec![entry(730, "Counter-Strike 2")]);
        assert_eq!(catalog.get(730).unwrap().name, "Counter-Strike 2");
        assert_eq!(catalog.get(440).unwrap().name, "Team Fortress 2");
        assert_eq!(catalog.entries[0].appid, 730);
    }

    #[test]
    fn tools_and_dlc_are_not_games() {
        assert!(!Entry {
            acquired: None,
            minutes: None,
            appid: 228980,
            name: "Steamworks Common Redistributables".into(),
            kind: "Tool".into(),
            family: false,
            details: Default::default(),
        }
        .is_game());
        let demo = Entry {
            acquired: None,
            minutes: None,
            appid: 1235,
            name: "Une démo".into(),
            kind: "Demo".into(),
            family: false,
            details: Default::default(),
        };
        assert!(demo.is_demo());
        assert!(!demo.is_game());
        assert!(!Entry {
            appid: 1236,
            kind: "DLC".into(),
            ..Default::default()
        }
        .is_demo());
        assert!(!Entry {
            acquired: None,
            minutes: None,
            appid: 1234,
            ..Default::default()
        }
        .is_game());
        assert_eq!(
            Entry {
                acquired: None,
                minutes: None,
                appid: 1234,
                ..Default::default()
            }
            .display_name(),
            "App 1234"
        );
    }

    #[test]
    fn only_the_owned_extras_of_a_game_are_proposed() {
        let mut game = entry(3658110, "Un jeu");
        // Le jeu déclare trois contenus ; le compte n'en possède que deux.
        game.details.dlc_ids = vec![100, 200, 300];
        let catalog = Catalog {
            version: VERSION,
            dlcs: vec![
                DlcEntry {
                    appid: 300,
                    name: "Troisième extension".into(),
                    install: 2_000,
                    download: 1_000,
                },
                DlcEntry {
                    appid: 100,
                    name: "Première extension".into(),
                    install: 4_000,
                    download: 2_000,
                },
            ],
            ..Default::default()
        };
        let owned = catalog.dlcs_of(&game);
        // L'ordre du jeu est gardé, pas celui du catalogue.
        assert_eq!(
            owned
                .iter()
                .map(|dlc| dlc.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Première extension", "Troisième extension"]
        );
        assert_eq!(owned[0].download, 2_000);
        assert!(catalog.dlc(200).is_none());
    }

    #[test]
    fn old_catalog_is_not_current() {
        let mut catalog = Catalog::default();
        assert!(!catalog.is_current());
        catalog.version = VERSION;
        assert!(catalog.is_current());
    }

    #[test]
    fn roundtrip_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("steam-catalog.json");
        let mut catalog = Catalog {
            version: VERSION,
            account: "bob".into(),
            updated: 42,
            entries: vec![entry(440, "Team Fortress 2")],
            dlcs: vec![DlcEntry {
                appid: 900,
                name: "Extension".into(),
                install: 10,
                download: 8,
            }],
            ..Default::default()
        };
        // La lecture complète les illustrations : l'aller-retour se compare à
        // un catalogue déjà complété, sinon il ne peut pas être égal.
        catalog.fill_missing_art();
        catalog.save(&path).unwrap();
        assert_eq!(Catalog::load(&path), catalog);
        assert_eq!(
            Catalog::load(&dir.path().join("absent.json")),
            Catalog::default()
        );
    }

    #[test]
    fn missing_art_is_derived_from_the_appid() {
        let mut catalog = Catalog {
            entries: vec![entry(1_092_660, "Blair Witch")],
            ..Default::default()
        };
        catalog.fill_missing_art();
        assert_eq!(
            catalog.get(1_092_660).unwrap().details.header_image,
            "https://cdn.cloudflare.steamstatic.com/steam/apps/1092660/header.jpg"
        );
    }

    #[test]
    fn a_stamped_date_survives_a_refresh() {
        let mut catalog = Catalog {
            entries: vec![entry(1_092_660, "Blair Witch")],
            ..Default::default()
        };
        catalog.stamped.insert(1_092_660, 1_700_000_000);
        let mut guessed = entry(1_092_660, "Blair Witch");
        guessed.acquired = Some(1_600_000_000);
        catalog.absorb(vec![guessed]);
        assert_eq!(
            catalog.get(1_092_660).and_then(|found| found.acquired),
            Some(1_700_000_000),
            "une date devinée ne remplace pas une date d'activation"
        );
    }

    #[test]
    fn absorb_keeps_a_known_date() {
        let mut dated = entry(1_092_660, "Blair Witch");
        dated.acquired = Some(1_700_000_000);
        let mut catalog = Catalog {
            entries: vec![dated],
            ..Default::default()
        };
        catalog.absorb(vec![entry(1_092_660, "Blair Witch")]);
        assert_eq!(
            catalog.get(1_092_660).and_then(|found| found.acquired),
            Some(1_700_000_000),
            "une relève sans date ne doit pas effacer la date connue"
        );
    }

    #[test]
    fn absorb_adds_a_game_absent_from_the_catalog() {
        let mut catalog = Catalog {
            account: "bob".into(),
            entries: vec![entry(440, "Team Fortress 2")],
            ..Default::default()
        };
        catalog.absorb(vec![entry(1_092_660, "Blair Witch")]);
        assert_eq!(catalog.entries.len(), 2, "le jeu de la clé est ajouté");
        assert_eq!(
            catalog.get(1_092_660).map(|found| found.name.as_str()),
            Some("Blair Witch")
        );
    }
}
