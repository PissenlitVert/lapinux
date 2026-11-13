//! Langues de l'interface et catalogues de traduction.
//!
//! Chaque langue est un catalogue TOML embarqué (`crates/core/locales/`) :
//! une clé stable désigne un libellé, et le catalogue actif fournit le texte.
//! Le français est la langue source ; une clé absente d'une autre langue
//! retombe sur le français, jamais sur la clé.
//!
//! Ajouter une langue : déposer le fichier, l'inscrire dans [`LANGUAGES`] et
//! dans `SOURCES`. Ajouter un libellé : la même clé dans tous les catalogues.
//! Les tests de ce module refusent une clé manquante ou un placeholder
//! divergent, si bien qu'une traduction oubliée casse la suite.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::LazyLock;

/// Langues proposées par l'interface, dans l'ordre du menu des réglages.
pub const LANGUAGES: &[(&str, &str)] = &[("fr", "Français"), ("en", "English")];

/// Catalogues embarqués. L'ordre fixe l'indice stocké dans `ACTIVE` ; « fr »
/// vient en tête, langue de l'interface tant qu'aucune configuration n'est lue.
const SOURCES: &[(&str, &str)] = &[
    ("fr", include_str!("../locales/fr.toml")),
    ("en", include_str!("../locales/en.toml")),
];

/// Langue source : celle vers laquelle retombe une traduction absente.
const SOURCE: &str = "fr";

struct Catalogue {
    entries: HashMap<String, String>,
}

impl Catalogue {
    /// Lit un catalogue TOML. Un catalogue mal formé est un défaut de
    /// fabrication : le programme s'arrête au premier libellé demandé, avec la
    /// clé fautive, plutôt que d'afficher des clés brutes.
    fn parse(source: &str) -> Self {
        let value: toml::Value = toml::from_str(source).expect("catalogue de langue illisible");
        let mut entries = HashMap::new();
        flatten("", &value, &mut entries);
        Self { entries }
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(String::as_str)
    }
}

/// Aplatit les tables TOML en clés pointées : `[library.filter]` + `missing`
/// donne `library.filter.missing`.
fn flatten(prefix: &str, value: &toml::Value, out: &mut HashMap<String, String>) {
    match value {
        toml::Value::Table(table) => {
            for (name, child) in table {
                let key = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}.{name}")
                };
                flatten(&key, child, out);
            }
        }
        toml::Value::String(text) => {
            out.insert(prefix.to_string(), text.clone());
        }
        other => panic!(
            "catalogue de langue : « {prefix} » doit être une chaîne, pas {}",
            other.type_str()
        ),
    }
}

/// Catalogues chargés une seule fois, au premier libellé demandé.
static CATALOGUES: LazyLock<Vec<(&'static str, Catalogue)>> = LazyLock::new(|| {
    SOURCES
        .iter()
        .map(|(code, source)| (*code, Catalogue::parse(source)))
        .collect()
});

/// Indice de la langue active dans `SOURCES`.
static ACTIVE: AtomicU8 = AtomicU8::new(0);

/// Langue imposée au démarrage par l'environnement, pour lancer toute la
/// suite de tests dans une langue donnée (`LAPINUX_LANG=en`). Une valeur
/// inconnue est ignorée : la langue de repli reste le français.
static ENV_LANGUAGE: std::sync::Once = std::sync::Once::new();

fn apply_env_language() -> bool {
    ENV_LANGUAGE.call_once(|| {
        if let Ok(code) = std::env::var("LAPINUX_LANG") {
            if let Some(index) = index_of(&normalize(&code)) {
                ACTIVE.store(index, Ordering::Relaxed);
            }
        }
    });
    std::env::var_os("LAPINUX_LANG").is_some()
}

fn apply_env_override() -> bool {
    apply_env_language()
}

/// Les tests changent la langue globale : sans verrou, deux tests qui la
/// règlent en même temps se marchent dessus. En production, seul le fil de
/// configuration appelle `set`, une fois au démarrage et à chaque sauvegarde.
#[cfg(test)]
static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn lock_language() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn catalogue(code: &str) -> Option<&'static Catalogue> {
    CATALOGUES
        .iter()
        .find(|(name, _)| *name == code)
        .map(|(_, catalogue)| catalogue)
}

fn index_of(code: &str) -> Option<u8> {
    SOURCES
        .iter()
        .position(|(name, _)| *name == code)
        .map(|index| index as u8)
}

/// Langue de l'interface déduite du bureau : `LANG`, `LC_MESSAGES` puis
/// `LC_ALL`, dans l'ordre où les programmes les consultent. Tout ce qui n'est
/// pas français devient anglais — l'anglais est le repli universel.
pub fn system_language() -> String {
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        let Ok(value) = std::env::var(key) else {
            continue;
        };
        let code = value
            .split(['.', '@'])
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if code.is_empty() {
            continue;
        }
        return if code.starts_with("fr") {
            "fr".into()
        } else {
            "en".into()
        };
    }
    "en".into()
}

/// Ramène une valeur de configuration à une langue gérée. Une valeur vide
/// reprend la locale du système : c'est ce que reçoit une installation neuve.
pub fn normalize(code: &str) -> String {
    let code = code.trim().to_ascii_lowercase();
    if code.is_empty() {
        return system_language();
    }
    if code.starts_with("fr") {
        "fr".into()
    } else {
        "en".into()
    }
}

/// Installe la langue active. Appelé au démarrage et à chaque configuration
/// enregistrée. Une langue inconnue laisse la langue courante en place.
pub fn set(code: &str) {
    // `LAPINUX_LANG` fige la langue : une suite de tests peut ainsi tourner
    // entièrement en anglais sans qu'une configuration chargée en cours de
    // route ne la ramène au français.
    if apply_env_override() {
        return;
    }
    if let Some(index) = index_of(&normalize(code)) {
        ACTIVE.store(index, Ordering::Relaxed);
    }
}

/// Variante de test : installe une langue même quand `LAPINUX_LANG` fige la
/// langue de la suite. Réservée aux tests du module, qui vérifient les deux
/// catalogues et doivent pouvoir basculer entre eux.
#[cfg(test)]
fn set_forced(code: &str) {
    if let Some(index) = index_of(&normalize(code)) {
        ACTIVE.store(index, Ordering::Relaxed);
    }
}

/// Code de la langue active (`fr`, `en`).
pub fn active() -> &'static str {
    apply_env_language();
    SOURCES[usize::from(ACTIVE.load(Ordering::Relaxed))].0
}

/// Vrai quand l'interface parle français.
pub fn french() -> bool {
    active() == "fr"
}

/// Libellé d'une clé dans la langue active.
///
/// Repli dans l'ordre : langue active, puis français (langue source), puis la
/// clé elle-même — visible en développement, jamais silencieux.
pub fn t(key: &'static str) -> &'static str {
    catalogue(active())
        .and_then(|catalogue| catalogue.get(key))
        .or_else(|| catalogue(SOURCE).and_then(|catalogue| catalogue.get(key)))
        .unwrap_or(key)
}

/// Libellé avec ses valeurs : chaque `{nom}` est remplacé par `args`.
///
/// Les valeurs arrivent déjà formatées (unité, pourcentage, nom de fichier) :
/// le catalogue ne connaît que la phrase et l'ordre de ses trous.
pub fn t_args(key: &'static str, args: &[(&str, &str)]) -> String {
    let mut text = t(key).to_string();
    for (name, value) in args {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}

/// Libellé avec ses valeurs positionnelles : chaque `{}` de la phrase est
/// remplacé, dans l'ordre, par un des textes déjà formatés de `args`.
///
/// Sert aux messages qui mêlent une phrase traduite et des valeurs
/// (compteur, nom de fichier) sans jamais concaténer de grammaire à la main :
/// l'ordre des trous appartient au catalogue, pas au code.
pub fn t_fmt(key: &'static str, args: &[String]) -> String {
    let mut text = t(key).to_string();
    for value in args {
        match text.find("{}") {
            Some(at) => text.replace_range(at..at + 2, value),
            None => break,
        }
    }
    text
}

/// Libellé d'une clé dans une langue donnée, avec le même repli que [`t`].
/// Sert aux tests et aux outils qui affichent une langue précise.
pub fn t_in(code: &str, key: &'static str) -> &'static str {
    catalogue(&normalize(code))
        .and_then(|catalogue| catalogue.get(key))
        .or_else(|| catalogue(SOURCE).and_then(|catalogue| catalogue.get(key)))
        .unwrap_or(key)
}

/// Ponctuation décimale de la langue active : virgule en français, point
/// ailleurs. Le nombre ne change pas, seul le séparateur suit la langue.
pub fn decimal(text: impl AsRef<str>) -> String {
    let text = text.as_ref();
    if active() == "fr" {
        text.replace('.', ",")
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalogues() -> Vec<(&'static str, &'static Catalogue)> {
        CATALOGUES
            .iter()
            .map(|(code, catalogue)| (*code, catalogue))
            .collect()
    }

    /// Placeholders d'un libellé, dans l'ordre d'apparition : `{count}`.
    fn placeholders(text: &str) -> Vec<String> {
        let mut names = Vec::new();
        let mut rest = text;
        while let Some(start) = rest.find('{') {
            let after = &rest[start + 1..];
            let Some(end) = after.find('}') else {
                break;
            };
            names.push(after[..end].to_string());
            rest = &after[end + 1..];
        }
        names
    }

    #[test]
    fn une_locale_inconnue_devient_anglais() {
        assert_eq!(normalize("de_DE.UTF-8"), "en");
        assert_eq!(normalize("en_GB.UTF-8"), "en");
        assert_eq!(normalize("fr_FR.UTF-8"), "fr");
        assert_eq!(normalize("fr"), "fr");
    }

    #[test]
    fn les_catalogues_partagent_les_memes_cles() {
        let source = catalogue(SOURCE).expect("catalogue source");
        for (code, catalogue) in catalogues() {
            for key in source.entries.keys() {
                assert!(
                    catalogue.entries.contains_key(key),
                    "« {key} » manque dans le catalogue {code}"
                );
            }
            for key in catalogue.entries.keys() {
                assert!(
                    source.entries.contains_key(key),
                    "« {key} » du catalogue {code} n'existe pas dans la source"
                );
            }
        }
    }

    #[test]
    fn les_placeholders_correspondent_d_une_langue_a_l_autre() {
        let source = catalogue(SOURCE).expect("catalogue source");
        for (code, catalogue) in catalogues() {
            for (key, text) in &source.entries {
                let translated = catalogue.entries.get(key).expect("clé vérifiée");
                assert_eq!(
                    placeholders(text),
                    placeholders(translated),
                    "placeholders de « {key} » différents en {code}"
                );
            }
        }
    }

    #[test]
    fn aucun_libelle_n_est_vide() {
        for (code, catalogue) in catalogues() {
            for (key, text) in &catalogue.entries {
                assert!(!text.trim().is_empty(), "« {key} » vide en {code}");
            }
        }
    }

    #[test]
    fn une_cle_traduite_suit_la_langue_active() {
        let _language = lock_language();
        let previous = active().to_string();
        set_forced("fr");
        assert_eq!(t("page.library"), "Bibliothèque");
        set_forced("en");
        assert_eq!(t("page.library"), "Library");
        assert_eq!(active(), "en");
        // La langue de départ des tests ne doit pas dépendre de la locale du
        // bureau : on remet le français, langue des autres tests d'interface.
        set_forced("fr");
    }

    #[test]
    fn une_cle_inconnue_rend_la_cle() {
        let _language = lock_language();
        let previous = active().to_string();
        set_forced("fr");
        assert_eq!(t("cle.inexistante"), "cle.inexistante");
    }

    #[test]
    fn les_nombres_suivent_la_langue() {
        let _language = lock_language();
        let previous = active().to_string();
        set_forced("fr");
        assert_eq!(decimal("12.5"), "12,5");
        set_forced("en");
        assert_eq!(decimal("12.5"), "12.5");
        set_forced("fr");
    }

    #[test]
    fn les_valeurs_remplissent_les_placeholders() {
        let _language = lock_language();
        let previous = active().to_string();
        set_forced("fr");
        assert_eq!(
            t_args("library.filter.installed", &[("installed", "12")]),
            "Installés · 12"
        );
        set_forced("en");
        assert_eq!(
            t_args("library.filter.installed", &[("installed", "12")]),
            "Installed · 12"
        );
        set_forced("fr");
    }

    #[test]
    fn chaque_langue_a_un_libelle() {
        for (code, label) in LANGUAGES {
            assert!(index_of(code).is_some(), "{code} absente de SOURCES");
            assert!(!label.trim().is_empty());
        }
    }

    /// Toutes les clés demandées par le code existent dans le catalogue
    /// source : une faute de frappe ne peut pas retomber silencieusement sur
    /// la clé brute à l'écran.
    #[test]
    fn toutes_les_cles_utilisees_existent() {
        fn scan(dir: &std::path::Path, out: &mut Vec<(std::path::PathBuf, String)>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    scan(&path, out);
                } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                    let Ok(text) = std::fs::read_to_string(&path) else {
                        continue;
                    };
                    for call in ["i18n::t(\"", "i18n::t_args(\""] {
                        let mut rest = text.as_str();
                        while let Some(index) = rest.find(call) {
                            let after = &rest[index + call.len()..];
                            let Some(end) = after.find('"') else {
                                break;
                            };
                            out.push((path.clone(), after[..end].to_string()));
                            rest = &after[end..];
                        }
                    }
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut found = Vec::new();
        scan(&root.join("crates"), &mut found);
        let source = catalogue(SOURCE).expect("catalogue source");
        let mut missing: Vec<String> = found
            .iter()
            .filter(|(_, key)| !source.entries.contains_key(key))
            .map(|(path, key)| format!("{} : {key}", path.display()))
            .collect();
        missing.sort();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "clés inconnues :\n{}",
            missing.join("\n")
        );
    }
}
