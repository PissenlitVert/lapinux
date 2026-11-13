//! Choix des dépôts, plateformes et langues.

use super::*;

/// Poids publié d'un contenu additionnel : ses propres dépôts, que la lecture
/// d'un jeu de base écarte. La plateforme suit celle de l'installation.
/// Poids de chaque contenu additionnel, lu dans les dépôts du **jeu parent**.
///
/// PICS ne décrit pas les dépôts d'un contenu dans sa propre fiche — elle ne
/// porte que son nom : les dépôts d'un contenu vivent dans la fiche du jeu, avec
/// un champ `dlcappid`. C'est de là que venait le « poids inconnu » : on
/// demandait les dépôts au mauvais endroit.
/// Poids des contenus d'une fiche de jeu, prêts à rejoindre la table du
/// catalogue.
pub(super) fn dlc_sizes_of(root: &KeyValue) -> HashMap<u32, lapinux_core::model::SteamSize> {
    dlc_sizes(app_section(root))
}

pub(crate) fn dlc_sizes(app: &KeyValue) -> HashMap<u32, lapinux_core::model::SteamSize> {
    let mut sizes: HashMap<u32, lapinux_core::model::SteamSize> = HashMap::new();
    // Une préférence par système, comme pour le jeu de base : Linux d'abord,
    // puis Windows et macOS pour les contenus qui n'en publient qu'un.
    for platform in ["linux", "windows", "macos"] {
        for (id, size) in dlc_sizes_on(app, platform) {
            sizes.entry(id).or_insert(size);
        }
    }
    sizes
}

fn dlc_sizes_on(app: &KeyValue, platform: &str) -> HashMap<u32, lapinux_core::model::SteamSize> {
    let mut sizes: HashMap<u32, lapinux_core::model::SteamSize> = HashMap::new();
    let Some(depots) = app.get("depots") else {
        return sizes;
    };
    let KvValue::Children(depots) = &depots.value else {
        return sizes;
    };
    for (key, depot) in depots {
        let Some(dlc) = number(depot.get("dlcappid")) else {
            continue;
        };
        if key.parse::<u32>().is_err() || depot.get("sharedinstall").is_some() {
            continue;
        }
        if let Some(oslist) = depot
            .get("config")
            .and_then(|config| config.get("oslist"))
            .and_then(KeyValue::as_str)
        {
            let wanted = oslist
                .split(',')
                .map(str::trim)
                .any(|os| os.eq_ignore_ascii_case(platform));
            if !wanted {
                continue;
            }
        }
        let Some(manifest) = depot.get("manifests").and_then(|all| all.get("public")) else {
            continue;
        };
        let install = number(manifest.get("size")).unwrap_or(0);
        let download = number(manifest.get("download")).unwrap_or(0);
        if install == 0 && download == 0 {
            continue;
        }
        let entry = sizes
            .entry(u32::try_from(dlc).unwrap_or_default())
            .or_default();
        entry.install += install;
        entry.download += download;
    }
    sizes
}

pub(crate) fn dlc_size(root: &KeyValue) -> Option<lapinux_core::model::SteamSize> {
    // (la fiche du DLC ne porte pas de dépôts d'édition : rien à départager)
    let app = app_section(root);
    let language = default_language();
    platform_size(app, "windows", None, None, language, true)
        .or_else(|| platform_size(app, "linux", None, None, language, true))
        .or_else(|| platform_size(app, "macos", None, None, language, true))
}

#[derive(Debug, Clone)]
pub(super) struct DepotPlan {
    pub(super) depot_id: u32,
    pub(super) manifest_id: u64,
    pub(super) from_app: Option<u32>,
}

/// Langue de doublage que Steam utiliserait, lue dans le registre du client.
///
/// Le protocole publie l'inventaire des langues d'un jeu (dépôts de doublage,
/// `supported_languages`, `baselanguages`) mais **pas** celle qui sera
/// retenue : ce choix appartient au client, qui l'envoie lui-même au CM dans
/// `CMsgClientLogon.client_language`. On lit donc le réglage que ce client
/// garde (`HKCU/Software/Valve/Steam/language` du `registry.vdf`) : c'est la
/// langue dans laquelle Steam lance réellement les jeux, pas celle du bureau.
pub(super) fn language_from_registry(text: &str) -> Option<&'static str> {
    let root = crate::vdf::parse(text).ok()?;
    let registry = root.obj("Registry")?;
    let machine = registry.obj("HKCU")?;
    let valve = machine.obj("Software")?.obj("Valve")?;
    let steam = valve.obj("Steam")?;
    language_code(steam.str("language")?)
}

/// Langue choisie par le client Steam, s'il est installé dans une racine
/// connue. `None` sur une machine sans client Steam.
/// Échappement d'un terme pour l'URL de recherche : lettres, chiffres et
/// ponctuation sûre passent tels quels, le reste devient `%XX`.
fn query_escape(term: &str) -> String {
    let mut out = String::with_capacity(term.len());
    for byte in term.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            b' ' => out.push_str("%20"),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Premier jeu d'un paquet de licences, demandé à la boutique : une clé
/// activée ne donne ni appid ni nom, mais sa licence porte son paquet.
pub fn package_app(package_id: u32) -> Option<(u32, String)> {
    crate::shared::block_on(async move {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .ok()?;
        let url = format!(
            "https://store.steampowered.com/api/packagedetails/?packageids={package_id}&l=french&cc=FR"
        );
        let response = client.get(url).send().await.ok()?;
        let value: serde_json::Value = response.json().await.ok()?;
        let apps = value
            .get(package_id.to_string())?
            .get("data")?
            .get("apps")?
            .as_array()?;
        for app in apps {
            let Some(id) = app.get("id").and_then(|id| id.as_u64()) else {
                continue;
            };
            let Some(name) = app.get("name").and_then(|name| name.as_str()) else {
                continue;
            };
            return Some((id as u32, name.to_string()));
        }
        None
    })
}

/// Appid du jeu qui porte ce nom, demandé à la recherche de la boutique
/// Steam : une clé activée ne joint pas toujours l'appid de son produit, et le
/// catalogue a besoin de lui pour ranger la fiche.
pub fn search_appid(name: &str) -> Option<(u32, String)> {
    let term = name.trim().to_string();
    if term.is_empty() {
        return None;
    }
    crate::shared::block_on(async move {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .ok()?;
        // Le client de ce module n'expose pas les paramètres d'URL : la
        // recherche part avec un terme encodé à la main.
        let url = format!(
            "https://store.steampowered.com/api/storesearch/?term={}&l=french&cc=FR",
            query_escape(&term)
        );
        let response = client.get(url).send().await.ok()?;
        let value: serde_json::Value = response.json().await.ok()?;
        let items = value.get("items")?.as_array()?;
        let mut fallback: Option<(u32, String)> = None;
        for item in items {
            let Some(appid) = item.get("id").and_then(|id| id.as_u64()) else {
                continue;
            };
            let found = item
                .get("name")
                .and_then(|name| name.as_str())
                .unwrap_or_default();
            if found.eq_ignore_ascii_case(&term) {
                return Some((appid as u32, found.to_string()));
            }
            if fallback.is_none() {
                fallback = Some((appid as u32, found.to_string()));
            }
        }
        fallback
    })
}

pub fn steam_language() -> Option<&'static str> {
    let mut candidates: Vec<std::path::PathBuf> = lapinux_core::paths::steam_roots()
        .into_iter()
        .map(|root| root.join("registry.vdf"))
        .collect();
    candidates.push(lapinux_core::paths::home_dir().join(".steam/registry.vdf"));
    candidates
        .into_iter()
        .find_map(|path| language_from_registry(&std::fs::read_to_string(path).ok()?))
}

/// Langue choisie dans les réglages de Lapinux (index dans
/// [`lapinux_core::lang::GAME_LANGUAGES`], `usize::MAX` pour « comme Steam »).
///
/// Elle prime sur celle du client : c'est elle qui décide des dépôts de
/// doublage téléchargés et du poids annoncé, donc de ce que le disque reçoit
/// vraiment.
static OVERRIDE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(usize::MAX);

/// Impose une langue de jeu à tout le moteur ; `None` rend la main au client
/// Steam puis au bureau. Appelée par le daemon à chaque configuration.
pub fn set_language_override(code: Option<&str>) {
    use std::sync::atomic::Ordering;
    let index = code
        .and_then(|code| {
            let code = code.trim();
            lapinux_core::lang::GAME_LANGUAGES
                .iter()
                .position(|language| language.code.eq_ignore_ascii_case(code))
        })
        .unwrap_or(usize::MAX);
    OVERRIDE.store(index, Ordering::Relaxed);
}

/// Langue imposée par les réglages, s'il y en a une.
pub fn language_override() -> Option<&'static str> {
    use std::sync::atomic::Ordering;
    let index = OVERRIDE.load(Ordering::Relaxed);
    lapinux_core::lang::GAME_LANGUAGES
        .get(index)
        .map(|language| language.code)
}

/// Langue de jeu par défaut : celle des réglages si elle est fixée, sinon
/// celle du client Steam, sinon celle du bureau. La lecture du client est
/// mémorisée : la fiche de chaque jeu la demande.
pub fn default_language() -> Option<&'static str> {
    if let Some(code) = language_override() {
        return Some(code);
    }
    use std::sync::OnceLock;
    static LANGUAGE: OnceLock<Option<&'static str>> = OnceLock::new();
    *LANGUAGE.get_or_init(|| steam_language().or_else(system_language))
}

/// Libellé français de cette langue, pour la fiche du jeu.
pub fn default_language_label() -> Option<String> {
    default_language().map(|code| language_label(code))
}

/// Code de dépôt Steam d'une langue, accepté en locale (`fr_FR.UTF-8`, `fr`)
/// comme en nom Steam (`french`, `koreana`).
pub(super) fn language_code(raw: &str) -> Option<&'static str> {
    let code = raw.trim();
    // Un nom Steam passe tel quel ; une locale se réduit à ses deux lettres.
    let lowered = code.to_ascii_lowercase();
    if let Some(found) = language_name(&lowered) {
        return Some(found);
    }
    let short = lowered
        .split(['_', '-', '.', '@'])
        .next()
        .unwrap_or_default();
    language_name(short)
}

/// Nom du bureau, dans le vocabulaire des dépôts Steam.
fn system_language() -> Option<&'static str> {
    let raw = std::env::var("LC_ALL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| std::env::var("LC_MESSAGES").ok())
        .or_else(|| std::env::var("LANG").ok())?;
    language_code(&raw)
}

/// Table locale courte → code de dépôt Steam, puis noms Steam tels quels.
fn language_name(code: &str) -> Option<&'static str> {
    Some(match code {
        "fr" => "french",
        "en" => "english",
        "de" => "german",
        "es" => "spanish",
        "it" => "italian",
        "pt" => "portuguese",
        "ru" => "russian",
        "pl" => "polish",
        "ja" => "japanese",
        "ko" => "koreana",
        "zh" => "schinese",
        "tr" => "turkish",
        "uk" => "ukrainian",
        "cs" => "czech",
        "da" => "danish",
        "nl" => "dutch",
        "fi" => "finnish",
        "no" => "norwegian",
        "sv" => "swedish",
        "hu" => "hungarian",
        "ro" => "romanian",
        "th" => "thai",
        "vi" => "vietnamese",
        "el" => "greek",
        "bg" => "bulgarian",
        // Noms déjà au vocabulaire Steam.
        "english" => "english",
        "french" => "french",
        "german" => "german",
        "spanish" => "spanish",
        "latam" => "latam",
        "italian" => "italian",
        "portuguese" => "portuguese",
        "brazilian" => "brazilian",
        "russian" => "russian",
        "polish" => "polish",
        "japanese" => "japanese",
        "koreana" => "koreana",
        "schinese" => "schinese",
        "tchinese" => "tchinese",
        "turkish" => "turkish",
        "ukrainian" => "ukrainian",
        "czech" => "czech",
        "danish" => "danish",
        "dutch" => "dutch",
        "finnish" => "finnish",
        "norwegian" => "norwegian",
        "swedish" => "swedish",
        "hungarian" => "hungarian",
        "romanian" => "romanian",
        "thai" => "thai",
        "vietnamese" => "vietnamese",
        "greek" => "greek",
        "bulgarian" => "bulgarian",
        _ => return None,
    })
}

/// Dépôts à installer : une seule langue de doublage, jamais les dépôts de
/// DLC (Steam les installe séparément, avec leur propre appid) ni les dépôts
/// partagés. Sans ce filtre, un jeu comme Cyberpunk annonçait 11 dépôts et
/// téléchargeait dix langues inutiles, soit ~50 Go de trop.
fn depot_plans(
    app: &KeyValue,
    platform: &str,
    language: Option<&str>,
    dlc: bool,
) -> Vec<DepotPlan> {
    let candidates = depot_candidates(app, platform, dlc);

    let available: Vec<&str> = candidates
        .iter()
        .filter_map(|(_, language)| language.as_deref())
        .collect();
    // Langue du client Steam si le jeu la propose, sinon l'anglais, sinon la
    // première par ordre alphabétique : le choix reste déterministe.
    let preferred = language
        .filter(|wanted| {
            available
                .iter()
                .any(|value| value.eq_ignore_ascii_case(wanted))
        })
        .map(str::to_string)
        .or_else(|| {
            available
                .iter()
                .find(|value| value.eq_ignore_ascii_case("english"))
                .map(|value| value.to_string())
        })
        .or_else(|| available.iter().min().map(|value| value.to_string()));

    candidates
        .into_iter()
        .filter(|(_, language)| language.is_none() || language.as_deref() == preferred.as_deref())
        .map(|(plan, _)| plan)
        .collect()
}

/// Tous les dépôts de contenu du jeu, avec leur langue éventuelle, avant le
/// choix de la langue de doublage.
fn depot_candidates(app: &KeyValue, platform: &str, dlc: bool) -> Vec<(DepotPlan, Option<String>)> {
    let Some(depots) = app.get("depots") else {
        return Vec::new();
    };
    let KvValue::Children(depots) = &depots.value else {
        return Vec::new();
    };
    let mut candidates: Vec<(DepotPlan, Option<String>)> = Vec::new();
    for (key, depot) in depots {
        let Ok(depot_id) = key.parse::<u32>() else {
            continue;
        };
        if depot_id == 0 {
            continue;
        }
        if depot.get("sharedinstall").is_some() || (!dlc && depot.get("dlcappid").is_some()) {
            continue;
        }
        if let Some(config) = depot.get("config") {
            if let Some(oslist) = config.get("oslist").and_then(KeyValue::as_str) {
                let wanted = oslist
                    .split(',')
                    .map(str::trim)
                    .any(|os| os.eq_ignore_ascii_case(platform));
                if !wanted {
                    continue;
                }
            }
        }
        let Some(manifests) = depot.get("manifests") else {
            continue;
        };
        let manifest = manifests
            .get("public")
            .map(|entry| entry.get("gid").unwrap_or(entry));
        let Some(manifest_id) = manifest.and_then(|entry| number(Some(entry))) else {
            continue;
        };
        let from_app = depot
            .get("depotfromapp")
            .and_then(|entry| number(Some(entry)))
            .or_else(|| {
                depot
                    .get("config")
                    .and_then(|config| config.get("depotfromapp"))
                    .and_then(|entry| number(Some(entry)))
            })
            .and_then(|value| u32::try_from(value).ok());
        let language = depot
            .get("config")
            .and_then(|config| config.get("language"))
            .and_then(KeyValue::as_str)
            .map(str::to_string);
        candidates.push((
            DepotPlan {
                depot_id,
                manifest_id,
                from_app,
            },
            language,
        ));
    }
    candidates.sort_by_key(|(plan, _)| plan.depot_id);
    candidates.dedup_by_key(|(plan, _)| plan.depot_id);
    candidates
}

pub(super) fn pick_platform(
    app: &KeyValue,
    language: Option<&str>,
    dlc: bool,
    preferred: Option<&str>,
) -> (&'static str, Vec<DepotPlan>) {
    // La version demandée passe d'abord ; sans choix, Lapinux lance les jeux
    // Windows avec Proton et ne prend Linux que si l'application n'a rien
    // pour Windows.
    if let Some(preferred) = preferred {
        let plans = depot_plans(app, preferred, language, dlc);
        if !plans.is_empty() {
            return (
                match preferred {
                    "linux" => "linux",
                    "macos" => "macos",
                    _ => "windows",
                },
                plans,
            );
        }
    }
    let windows = depot_plans(app, "windows", language, dlc);
    if !windows.is_empty() {
        return ("windows", windows);
    }
    let linux = depot_plans(app, "linux", language, dlc);
    if !linux.is_empty() {
        return ("linux", linux);
    }
    ("macos", depot_plans(app, "macos", language, dlc))
}

pub(super) fn safe_installdir(meta: &AppMeta, appid: u32) -> String {
    let raw = meta.installdir.trim();
    let clean = Path::new(raw)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    if !clean.is_empty() && clean != "." && clean != ".." {
        return clean;
    }
    let fallback = if meta.name.trim().is_empty() {
        format!("app-{appid}")
    } else {
        meta.name.clone()
    };
    let slug = lapinux_core::util::slugify(&fallback);
    if slug.is_empty() {
        format!("app-{appid}")
    } else {
        slug
    }
}

pub(super) fn directory_size(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                directory_size(&path)
            } else {
                entry.metadata().map(|meta| meta.len()).unwrap_or(0)
            }
        })
        .sum()
}
