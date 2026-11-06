//! Langues de jeu, telles que Steam les nomme dans ses dépôts.
//!
//! Steam désigne une langue par un nom (`french`, `schinese`, `brazilian`…),
//! et c'est ce nom que portent les dépôts de doublage : le choisir revient à
//! choisir les fichiers téléchargés. La même table donne la locale POSIX posée
//! au lancement, pour que le jeu (et Wine) parle la langue demandée.

/// Une langue jouable : son nom Steam, son libellé et sa locale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GameLanguage {
    /// Nom Steam de la langue : la clé que lisent les dépôts et PICS.
    pub code: &'static str,
    /// Libellé français, pour les listes de l'interface.
    pub label: &'static str,
    /// Locale POSIX (`fr_FR.UTF-8`), posée dans l'environnement du jeu.
    pub locale: &'static str,
}

/// Langues proposées par Lapinux, dans l'ordre des menus Steam.
pub const GAME_LANGUAGES: [GameLanguage; 28] = [
    GameLanguage {
        code: "english",
        label: "Anglais",
        locale: "en_US.UTF-8",
    },
    GameLanguage {
        code: "french",
        label: "Français",
        locale: "fr_FR.UTF-8",
    },
    GameLanguage {
        code: "german",
        label: "Allemand",
        locale: "de_DE.UTF-8",
    },
    GameLanguage {
        code: "spanish",
        label: "Espagnol (Espagne)",
        locale: "es_ES.UTF-8",
    },
    GameLanguage {
        code: "latam",
        label: "Espagnol (Amérique latine)",
        locale: "es_419.UTF-8",
    },
    GameLanguage {
        code: "italian",
        label: "Italien",
        locale: "it_IT.UTF-8",
    },
    GameLanguage {
        code: "portuguese",
        label: "Portugais (Portugal)",
        locale: "pt_PT.UTF-8",
    },
    GameLanguage {
        code: "brazilian",
        label: "Portugais (Brésil)",
        locale: "pt_BR.UTF-8",
    },
    GameLanguage {
        code: "russian",
        label: "Russe",
        locale: "ru_RU.UTF-8",
    },
    GameLanguage {
        code: "japanese",
        label: "Japonais",
        locale: "ja_JP.UTF-8",
    },
    GameLanguage {
        code: "koreana",
        label: "Coréen",
        locale: "ko_KR.UTF-8",
    },
    GameLanguage {
        code: "schinese",
        label: "Chinois simplifié",
        locale: "zh_CN.UTF-8",
    },
    GameLanguage {
        code: "tchinese",
        label: "Chinois traditionnel",
        locale: "zh_TW.UTF-8",
    },
    GameLanguage {
        code: "polish",
        label: "Polonais",
        locale: "pl_PL.UTF-8",
    },
    GameLanguage {
        code: "turkish",
        label: "Turc",
        locale: "tr_TR.UTF-8",
    },
    GameLanguage {
        code: "dutch",
        label: "Néerlandais",
        locale: "nl_NL.UTF-8",
    },
    GameLanguage {
        code: "swedish",
        label: "Suédois",
        locale: "sv_SE.UTF-8",
    },
    GameLanguage {
        code: "norwegian",
        label: "Norvégien",
        locale: "nb_NO.UTF-8",
    },
    GameLanguage {
        code: "danish",
        label: "Danois",
        locale: "da_DK.UTF-8",
    },
    GameLanguage {
        code: "finnish",
        label: "Finnois",
        locale: "fi_FI.UTF-8",
    },
    GameLanguage {
        code: "czech",
        label: "Tchèque",
        locale: "cs_CZ.UTF-8",
    },
    GameLanguage {
        code: "hungarian",
        label: "Hongrois",
        locale: "hu_HU.UTF-8",
    },
    GameLanguage {
        code: "romanian",
        label: "Roumain",
        locale: "ro_RO.UTF-8",
    },
    GameLanguage {
        code: "ukrainian",
        label: "Ukrainien",
        locale: "uk_UA.UTF-8",
    },
    GameLanguage {
        code: "greek",
        label: "Grec",
        locale: "el_GR.UTF-8",
    },
    GameLanguage {
        code: "bulgarian",
        label: "Bulgare",
        locale: "bg_BG.UTF-8",
    },
    GameLanguage {
        code: "thai",
        label: "Thaï",
        locale: "th_TH.UTF-8",
    },
    GameLanguage {
        code: "vietnamese",
        label: "Vietnamien",
        locale: "vi_VN.UTF-8",
    },
];

/// Langue d'après son nom Steam (insensible à la casse).
pub fn find(code: &str) -> Option<&'static GameLanguage> {
    let code = code.trim();
    if code.is_empty() {
        return None;
    }
    GAME_LANGUAGES
        .iter()
        .find(|language| language.code.eq_ignore_ascii_case(code))
}

/// Libellé français d'une langue ; vide si le nom Steam est inconnu.
pub fn label(code: &str) -> Option<&'static str> {
    find(code).map(|language| language.label)
}

/// Langue retenue pour un jeu : celle choisie pour lui, sinon celle des
/// réglages généraux, sinon `None` — le moteur garde alors la langue que Steam
/// ou le bureau imposent.
pub fn resolve(game: &str, global: &str) -> Option<&'static GameLanguage> {
    find(game).or_else(|| find(global))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_langues_se_lisent_par_leur_nom_steam() {
        assert_eq!(find("french").map(|l| l.label), Some("Français"));
        assert_eq!(find("FRENCH").map(|l| l.locale), Some("fr_FR.UTF-8"));
        assert_eq!(find("schinese").map(|l| l.label), Some("Chinois simplifié"));
        // Un nom inconnu ou vide n'impose rien.
        assert!(find("klingon").is_none());
        assert!(find("  ").is_none());
        assert_eq!(label("brazilian"), Some("Portugais (Brésil)"));
    }

    #[test]
    fn la_langue_du_jeu_passe_devant_celle_des_reglages() {
        assert_eq!(
            resolve("japanese", "french").map(|l| l.code),
            Some("japanese")
        );
        assert_eq!(resolve("", "french").map(|l| l.code), Some("french"));
        assert!(resolve("", "").is_none());
    }

    #[test]
    fn chaque_langue_a_une_locale_et_un_libelle() {
        for language in GAME_LANGUAGES {
            assert!(language.locale.contains(".UTF-8"), "{language:?}");
            assert!(!language.label.is_empty());
            assert!(language.code.chars().all(|c| c.is_ascii_lowercase()));
        }
    }
}
