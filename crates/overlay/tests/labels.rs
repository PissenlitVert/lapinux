//! Garde des libellés de l'overlay : chaque clé `overlay.*` utilisée par le
//! code existe dans les deux catalogues, avec les mêmes placeholders, et
//! change bien de texte entre le français et l'anglais.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn catalogues() -> (BTreeMap<String, String>, BTreeMap<String, String>) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/locales");
    let lire = |nom: &str| -> BTreeMap<String, String> {
        let texte = std::fs::read_to_string(root.join(nom))
            .unwrap_or_else(|error| panic!("{nom} illisible : {error}"));
        let valeur: toml::Value =
            toml::from_str(&texte).unwrap_or_else(|error| panic!("{nom} invalide : {error}"));
        let mut sortie = BTreeMap::new();
        aplatir("", valeur.as_table().expect("table TOML"), &mut sortie);
        sortie
    };
    (lire("fr.toml"), lire("en.toml"))
}

fn aplatir(prefixe: &str, table: &toml::value::Table, sortie: &mut BTreeMap<String, String>) {
    for (cle, valeur) in table {
        let cle = if prefixe.is_empty() {
            cle.clone()
        } else {
            format!("{prefixe}.{cle}")
        };
        match valeur {
            toml::Value::Table(table) => aplatir(&cle, table, sortie),
            toml::Value::String(texte) => {
                sortie.insert(cle, texte.clone());
            }
            autre => panic!("la clé {cle} n'est pas une chaîne : {}", autre.type_str()),
        }
    }
}

fn placeholders(texte: &str) -> BTreeSet<String> {
    let mut sortie = BTreeSet::new();
    let mut reste = texte;
    while let Some(debut) = reste.find('{') {
        let apres = &reste[debut + 1..];
        let Some(fin) = apres.find('}') else { break };
        sortie.insert(apres[..fin].to_string());
        reste = &apres[fin + 1..];
    }
    sortie
}

/// Toutes les clés `overlay.*` appelées dans les sources de l'overlay.
fn cles_utilisees() -> BTreeSet<String> {
    let racine = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut cles = BTreeSet::new();
    let mut fichiers: Vec<PathBuf> = std::fs::read_dir(&racine)
        .expect("dossier src lisible")
        .filter_map(|entree| entree.ok().map(|entree| entree.path()))
        .filter(|chemin| chemin.extension().and_then(|ext| ext.to_str()) == Some("rs"))
        .collect();
    fichiers.sort();
    for chemin in fichiers {
        let texte = std::fs::read_to_string(&chemin)
            .unwrap_or_else(|error| panic!("{} illisible : {error}", chemin.display()));
        for declaration in ["t(\"", "t_args(\"", "t_fmt(\""] {
            let mut reste = texte.as_str();
            while let Some(debut) = reste.find(declaration) {
                let apres = &reste[debut + declaration.len()..];
                let Some(fin) = apres.find('"') else { break };
                let cle = &apres[..fin];
                if cle.starts_with("overlay.") {
                    cles.insert(cle.to_string());
                }
                reste = &apres[fin + 1..];
            }
        }
    }
    cles
}

#[test]
fn tous_les_libelles_overlay_sont_traduits() {
    let (fr, en) = catalogues();
    let cles = cles_utilisees();
    assert!(
        cles.len() >= 100,
        "l'overlay doit passer par le catalogue : {} clés trouvées",
        cles.len()
    );
    // Textes volontairement identiques dans les deux langues.
    let identiques = ["overlay.pause", "overlay.inactive", "overlay.section_position"];
    for cle in &cles {
        let francais = fr
            .get(cle)
            .unwrap_or_else(|| panic!("clé absente du catalogue fr : {cle}"));
        let anglais = en
            .get(cle)
            .unwrap_or_else(|| panic!("clé absente du catalogue en : {cle}"));
        assert!(!francais.trim().is_empty(), "libellé fr vide : {cle}");
        assert!(!anglais.trim().is_empty(), "libellé en vide : {cle}");
        assert!(!anglais.starts_with("overlay."), "clé brute affichée : {cle}");
        // Le catalogue réellement branché ne rend jamais la clé brute.
        let cle_static: &'static str = Box::leak(cle.clone().into_boxed_str());
        let rendu = lapinux_core::i18n::t(cle_static);
        assert!(!rendu.is_empty(), "libellé vide rendu pour {cle}");
        assert_ne!(rendu, *cle, "clé brute rendue pour {cle}");
        assert_eq!(
            placeholders(francais),
            placeholders(anglais),
            "placeholders divergents pour {cle}"
        );
        if !identiques.contains(&cle.as_str()) {
            assert_ne!(
                francais, anglais,
                "la clé {cle} n'est pas traduite (même texte dans les deux langues)"
            );
        }
    }
}

#[test]
fn les_libelles_de_l_overlay_passent_en_anglais() {
    if std::env::var_os("LAPINUX_LANG").is_some() {
        // La langue est figée par l'environnement de la suite : rien à tester ici.
        return;
    }
    lapinux_core::i18n::set("en");
    let menu = lapinux_overlay::menu::MenuState::new("Portal 2");
    assert_eq!(menu.label(lapinux_overlay::menu::Item::Resume), "Resume");
    assert_eq!(menu.label(lapinux_overlay::menu::Item::Screenshot), "Screenshot");
    assert_eq!(menu.hint(lapinux_overlay::menu::Item::Profile), "Enter · list");
    assert_eq!(lapinux_overlay::menu::HudPosition::TopLeft.label(), "Top left");
    assert_eq!(lapinux_overlay::menu::HudLayout::Block.label(), "Block");
    assert!(menu.label(lapinux_overlay::menu::Item::QuitGame).starts_with("Quit "));
    assert_eq!(lapinux_core::hud::HudElement::Cores.label(), "Cores");
    lapinux_core::i18n::set("fr");
}

#[test]
fn les_catalogues_fr_et_en_ont_les_memes_cles_overlay() {
    let (fr, en) = catalogues();
    let cles_fr: BTreeSet<&String> = fr.keys().filter(|cle| cle.starts_with("overlay.")).collect();
    let cles_en: BTreeSet<&String> = en.keys().filter(|cle| cle.starts_with("overlay.")).collect();
    assert!(!cles_fr.is_empty());
    assert_eq!(cles_fr, cles_en, "les clés overlay divergent entre fr et en");
}
