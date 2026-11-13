//! Garde i18n : parité fr/en, clés appelées présentes, et aucun littéral
//! d'interface visible non traduit dans `crates/ui/src`.
//!
//! La liste d'exceptions est volontairement courte : elle ne contient que des
//! données externes, des identifiants internes et des arguments de commande.
//! Les noms propres ne sont pas des exceptions : ils sont admis comme textes
//! visibles, car ils ne se traduisent pas.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const FRENCH_ACCENTS: &[char] = &[
    'à', 'â', 'ä', 'ç', 'é', 'è', 'ê', 'ë', 'î', 'ï', 'ô', 'ö', 'ù', 'û', 'ü', 'ÿ', 'œ', 'æ', 'À',
    'Â', 'Ä', 'Ç', 'É', 'È', 'Ê', 'Ë', 'Î', 'Ï', 'Ô', 'Ö', 'Ù', 'Û', 'Ü', 'Ÿ', 'Œ', 'Æ', '«', '»',
];

const FRENCH_WORDS: &[&str] = &[
    "de",
    "le",
    "la",
    "les",
    "du",
    "et",
    "ce",
    "qui",
    "que",
    "ne",
    "un",
    "une",
    "des",
    "aux",
    "jeu",
    "jeux",
    "dépôt",
    "paramètres",
    "réglages",
    "boutique",
    "manette",
    "écran",
    "fenêtre",
    "sauvegarde",
    "téléchargement",
    "fichier",
    "dossier",
    "aucun",
    "aucune",
    "votre",
    "vous",
    "nous",
    "avec",
    "pour",
    "dans",
    "sans",
    "sur",
    "est",
    "sont",
    "pas",
    "plus",
    "tous",
    "toutes",
    "cette",
    "ces",
    "ton",
    "ta",
    "tes",
    "mon",
    "ma",
    "mes",
    "son",
    "sa",
    "ses",
    "mode",
    "mot",
    "se",
    "connecter",
    "copie",
    "commande",
    "restantes",
    "fourni",
    "cadence",
    "encodeur",
    "sommaire",
    "vitrines",
    "commentaires",
    "plateformes",
    "amis",
    "offres",
    "boutiques",
    "sources",
    "parcourir",
    "lecture",
    "enregistrement",
    "impossible",
    "connexion",
    "jeton",
    "mods",
    "direct",
    "bonjour",
    "merci",
    "oui",
    "non",
    "jouer",
    "cours",
    "envoyée",
    "haut-parleur",
];

/// Noms propres et symboles : jamais traduits, jamais des libellés français.
const PROPER_NOUNS: &[&str] = &[
    "Steam",
    "Epic Games",
    "Cloudflare",
    "GameScope",
    "Proton",
    "SteamGridDB",
    "SGDB",
    "LAPINUX",
    "Le Monde",
    "Nexus Mods",
    "Steam Workshop",
    "Linux",
    "Windows (Proton)",
    "H.264",
    "H.265 · HEVC",
    "appid",
    "×",
    "·",
    "★",
    "▎",
];

/// Exceptions justifiées : données externes, identifiants internes ou
/// arguments de commande — jamais un texte affiché à l'utilisateur.
const EXCEPTIONS: &[(&str, &str)] = &[
    ("~/Jeux/LapinuxLibrary", "chemin d'exemple du champ dossier"),
    ("aucun", "valeur de configuration historique, jamais affichée"),
    (
        "nettoyage des doublages",
        "phase héritée d'un ancien daemon",
    ),
    ("dépôt", "alias de phase lu depuis le protocole existant"),
    (
        "permission de présence",
        "note héritée d'une ancienne boutique",
    ),
    ("sources", "argument passé à pactl, jamais affiché"),
    ("boutiques", "identifiant interne de page"),
    ("lecture impossible", "état interne du lecteur"),
    ("connexion", "nom interne d'une commande"),
    ("mon-plugin", "identifiant généré pour un plugin"),
    ("mon-theme", "identifiant généré pour un thème"),
    ("id = \"mon-plugin\"", "ligne du modèle TOML généré"),
    (
        "https://www.lemonde.fr/jeux-video/rss_full.xml",
        "URL de flux externe, jamais affichée",
    ),
    (
        "https://www.clubic.com/feed/news-jeux-video.rss",
        "URL de flux externe, jamais affichée",
    ),
    (
        "https://www.01net.com/actualites/jeux-video/rss/",
        "URL de flux externe, jamais affichée",
    ),
];

fn flatten(prefix: &str, table: &toml::value::Table, out: &mut BTreeMap<String, String>) {
    for (key, value) in table {
        let full = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            toml::Value::Table(table) => flatten(&full, table, out),
            toml::Value::String(text) => {
                out.insert(full, text.clone());
            }
            _ => panic!("la clé {full} n'est pas une chaîne"),
        }
    }
}

fn catalogue(path: &Path) -> BTreeMap<String, String> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{} illisible : {error}", path.display()));
    let value: toml::Value = toml::from_str(&text)
        .unwrap_or_else(|error| panic!("{} invalide : {error}", path.display()));
    let mut out = BTreeMap::new();
    flatten("", value.as_table().expect("table TOML"), &mut out);
    out
}

fn placeholders(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else { break };
        out.insert(after[..end].to_string());
        rest = &after[end + 1..];
    }
    out
}

fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_proper_noun(text: &str) -> bool {
    PROPER_NOUNS.contains(&normalize(text).as_str())
}

fn exception(text: &str) -> Option<&'static str> {
    let text = normalize(text);
    EXCEPTIONS
        .iter()
        .find(|(value, _)| normalize(value) == text)
        .map(|(_, reason)| *reason)
}

fn french_word(text: &str) -> Option<String> {
    let mut word = String::new();
    for ch in text.to_lowercase().chars() {
        if ch.is_alphanumeric() || ch == '’' || ch == '\'' {
            word.push(ch);
        } else {
            if FRENCH_WORDS.contains(&word.as_str()) {
                return Some(word);
            }
            word.clear();
        }
    }
    if FRENCH_WORDS.contains(&word.as_str()) {
        return Some(word);
    }
    None
}

/// Littéraux d'une source, avec leur position et leur texte décodé.
fn literals(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        // Chaîne brute (`r"…"`, `r#"…"#`, `br#"…"#`) : son contenu est
        // souvent du HTML ou une expression régulière, pas un libellé.
        let raw = rest.strip_prefix('r').or_else(|| rest.strip_prefix("br"));
        if let Some(raw) = raw {
            let hashes = raw.chars().take_while(|ch| *ch == '#').count();
            if raw[hashes..].starts_with('"') {
                let closing = format!("\"{}", "#".repeat(hashes));
                let body = &raw[hashes + 1..];
                i += rest.len() - body.len();
                i += body
                    .find(&closing)
                    .map_or(body.len(), |end| end + closing.len());
                continue;
            }
        }
        // Littéral de caractère : `'"'` ne doit pas ouvrir une chaîne.
        if rest.starts_with('\'') {
            let mut chars = rest.chars();
            chars.next();
            let first = chars.next();
            let second = chars.next();
            if first == Some('\\') {
                if let Some(third) = chars.next() {
                    if chars.next() == Some('\'') {
                        i += 2 + third.len_utf8() + 1;
                        continue;
                    }
                }
            } else if second == Some('\'') {
                i += 1 + first.map_or(0, char::len_utf8) + 1;
                continue;
            }
        }
        if text.as_bytes()[i] != b'"' {
            i += text[i..].chars().next().map_or(1, char::len_utf8);
            continue;
        }
        let start = i;
        i += 1;
        let mut value = String::new();
        while i < text.len() {
            let ch = text[i..].chars().next().expect("caractère");
            match ch {
                '"' => {
                    i += 1;
                    break;
                }
                '\\' => {
                    i += 1;
                    let escaped = text[i..].chars().next().expect("échappement");
                    match escaped {
                        'n' => value.push('\n'),
                        't' => value.push('\t'),
                        'r' => value.push('\r'),
                        '\n' => {
                            i += 1;
                            while i < text.len() && text[i..].starts_with([' ', '\t', '\r', '\n']) {
                                i += text[i..].chars().next().map_or(1, char::len_utf8);
                            }
                            continue;
                        }
                        other => value.push(other),
                    }
                    i += escaped.len_utf8();
                }
                other => {
                    value.push(other);
                    i += other.len_utf8();
                }
            }
        }
        out.push((start, value));
    }
    out
}

fn is_comment(text: &str, start: usize) -> bool {
    let line_start = text[..start].rfind('\n').map_or(0, |index| index + 1);
    let before = &text[line_start..start];
    before.trim_start().starts_with("//")
        || before.trim_start().starts_with('*')
        || before.contains("//")
}

fn is_key(text: &str, start: usize) -> bool {
    let before = text[..start].trim_end();
    for function in ["t_args", "t_fmt", "t"] {
        let Some(prefix) = before.strip_suffix(&format!("{function}(")) else {
            continue;
        };
        // `hint_text(` finit aussi par « t( » : seul un appel dont le nom
        // commence à une frontière de jeton est une traduction.
        let previous = prefix.chars().last();
        if previous.map_or(true, |ch| !(ch.is_alphanumeric() || ch == '_')) {
            return true;
        }
    }
    false
}

fn is_direct_ui(text: &str, start: usize) -> bool {
    let before = normalize(&text[..start]);
    [
        "ui.label(",
        "ui.heading(",
        "ui.small(",
        "ui.strong(",
        "ui.monospace(",
        "muted( ui ,",
        "info_icon( ui ,",
        "Button::new( ui ,",
        "Label::new(",
        "RichText::new(",
        "secondary_button( ui ,",
        "settings_card( ui ,",
        "hint_box( ui ,",
        "badge( ui ,",
    ]
    .iter()
    .any(|pattern| before.ends_with(pattern))
}

fn source_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("dossier lisible") {
            let path = entry.expect("entrée").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs")
                && path.file_name().and_then(|name| name.to_str()) != Some("ui_tests.rs")
            {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

fn project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("racine du projet")
        .to_path_buf()
}

#[test]
fn les_catalogues_fr_et_en_ont_les_memes_cles_et_placeholders() {
    let root = project_root();
    let fr = catalogue(&root.join("crates/core/locales/fr.toml"));
    let en = catalogue(&root.join("crates/core/locales/en.toml"));
    assert!(!fr.is_empty());
    assert_eq!(
        fr.keys().collect::<Vec<_>>(),
        en.keys().collect::<Vec<_>>(),
        "les catalogues fr et en divergent"
    );
    for (key, text) in &fr {
        let other = en.get(key).expect("clé anglaise");
        assert_eq!(
            placeholders(text),
            placeholders(other),
            "placeholders différents pour {key}"
        );
        assert!(!text.trim().is_empty(), "{key} vide en fr");
        assert!(!other.trim().is_empty(), "{key} vide en en");
    }
}

#[test]
fn toutes_les_cles_appelees_par_l_interface_existent() {
    let root = project_root();
    let fr = catalogue(&root.join("crates/core/locales/fr.toml"));
    let mut missing = Vec::new();
    let mut count = 0;
    for path in source_files(&root.join("crates/ui/src")) {
        let text = std::fs::read_to_string(&path).expect("source lisible");
        let production = text.split("#[cfg(test)]").next().unwrap_or(&text);
        for (start, value) in literals(production) {
            if is_comment(production, start) || !is_key(production, start) {
                continue;
            }
            count += 1;
            if !fr.contains_key(&value) {
                missing.push(format!("{} : {value}", path.display()));
            }
        }
    }
    assert!(count > 100, "balayage des clés trop pauvre : {count}");
    assert!(
        missing.is_empty(),
        "clés absentes du catalogue :\n{}",
        missing.join("\n")
    );
}

#[test]
fn aucun_litteral_d_interface_francais_ne_reapparait() {
    let root = project_root();
    let mut failures = Vec::new();
    let mut used_exceptions = BTreeSet::new();
    let mut scanned = 0;
    for path in source_files(&root.join("crates/ui/src")) {
        let text = std::fs::read_to_string(&path).expect("source lisible");
        let production = text.split("#[cfg(test)]").next().unwrap_or(&text);
        for (start, value) in literals(production) {
            if is_comment(production, start) || is_key(production, start) {
                continue;
            }
            scanned += 1;
            if is_proper_noun(&value) {
                continue;
            }
            if let Some(reason) = exception(&value) {
                used_exceptions.insert(normalize(&value));
                let _ = reason;
                continue;
            }
            let line = production[..start].matches('\n').count() + 1;
            let accent = value.chars().any(|ch| FRENCH_ACCENTS.contains(&ch));
            let word = french_word(&value);
            if accent || word.is_some() || is_direct_ui(production, start) {
                let why = if let Some(word) = word {
                    format!("mot français « {word} »")
                } else if accent {
                    "accent français".to_string()
                } else {
                    "littéral passé directement à un helper d'affichage".to_string()
                };
                failures.push(format!(
                    "{}:{line} : {value:?} ({why}) — passe par le catalogue",
                    path.display()
                ));
            }
        }
    }
    assert!(
        scanned > 500,
        "balayage des littéraux trop pauvre : {scanned}"
    );
    let stale: Vec<_> = EXCEPTIONS
        .iter()
        .filter(|(value, _)| !used_exceptions.contains(&normalize(value)))
        .map(|(value, _)| *value)
        .collect();
    assert!(stale.is_empty(), "exceptions devenues inutiles : {stale:?}");
    assert!(
        failures.is_empty(),
        "littéraux d'interface non traduits :\n{}",
        failures.join("\n")
    );
}
