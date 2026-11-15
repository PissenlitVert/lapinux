//! Identité d'une manette : marque, famille et modèle.
//!
//! Les identifiants USB sont la source fiable : une DualShock 4 sous Linux
//! s'appelle souvent « Wireless Controller » et une Switch Pro « Pro
//! Controller ». Quand l'identifiant n'est pas connu (adaptateurs PS1/PS2
//! génériques, clones), on retombe sur le nom annoncé par evdev, puis sur le
//! nom brut.

/// Marque dont le logo accompagne le nom dans la liste.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Brand {
    PlayStation,
    Xbox,
    Nintendo,
    Steam,
    /// Manette sans marque identifiable, nommée d'après sa liaison.
    Generic,
}

impl Brand {
    pub fn label(self) -> &'static str {
        match self {
            Brand::PlayStation => "PlayStation",
            Brand::Xbox => "Xbox",
            Brand::Nintendo => "Nintendo",
            Brand::Steam => "Steam",
            Brand::Generic => lapinux_core::i18n::t("controllers.information.unknown"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PadIdentity {
    pub brand: Option<Brand>,
    /// Famille lisible, affichée à côté de la connexion : « PS5 », « Switch »…
    pub family: Option<&'static str>,
    /// Modèle officiel, plus court que le nom brut d'evdev.
    pub model: Option<&'static str>,
}

impl PadIdentity {
    /// Nom affiché : le modèle reconnu, sinon le nom brut de la manette.
    pub fn display_name<'a>(&self, raw: &'a str) -> &'a str {
        self.model.unwrap_or(raw)
    }
}

impl Default for PadIdentity {
    fn default() -> Self {
        Self {
            brand: None,
            family: None,
            model: None,
        }
    }
}

fn known(brand: Brand, family: &'static str, model: &'static str) -> PadIdentity {
    PadIdentity {
        brand: Some(brand),
        family: Some(family),
        model: Some(model),
    }
}

/// Reconnaît une manette par ses identifiants USB, puis par son nom.
///
/// Un gamepad qui ne correspond à aucune marque connue garde les libellés
/// Xbox (A/B/X/Y, View/Menu), mais sa marque et sa console restent inconnues :
/// on affiche « Inconnu » plutôt que d'inventer un modèle Xbox.
pub fn identify(vendor: u16, product: u16, name: &str) -> PadIdentity {
    // Nom donné par `describe` à un pad générique : la marque et la console
    // sont inconnues, même si le profil de boutons reste celui d'une Xbox.
    if name == lapinux_core::i18n::t("pad.radio_2_4ghz") {
        return PadIdentity {
            brand: Some(Brand::Generic),
            family: None,
            model: None,
        };
    }
    from_ids(vendor, product)
        .or_else(|| from_name(name))
        .unwrap_or(PadIdentity {
            brand: Some(Brand::Generic),
            family: None,
            model: None,
        })
}

/// Identité du pad virtuel créé par `steam-puck-bridge`. Il s'annonce en
/// Xbox 360 (`045e:028e`, « Microsoft X-Box 360 pad ») pour les jeux, mais la
/// manette reliée au Puck est une Steam Controller 2026.
pub fn steam_bridge_identity() -> PadIdentity {
    known(Brand::Steam, "Steam", "Steam Controller 2026")
}

fn from_ids(vendor: u16, product: u16) -> Option<PadIdentity> {
    use Brand::{Nintendo, PlayStation, Steam, Xbox};
    let identity = match (vendor, product) {
        // Sony
        (0x054c, 0x0ce6) => known(PlayStation, "PS5", "DualSense"),
        (0x054c, 0x0df2) => known(PlayStation, "PS5", "DualSense Edge"),
        (0x054c, 0x05c4 | 0x09cc | 0x0ba0) => known(PlayStation, "PS4", "DualShock 4"),
        (0x054c, 0x0268) => known(PlayStation, "PS3", "DualShock 3"),
        (0x054c, 0x03d5) => known(PlayStation, "PS3", "PlayStation Move"),
        (0x054c, 0x0cda) => known(PlayStation, "PS1", "PlayStation Classic"),
        // Microsoft
        // Le même identifiant sert aux vraies Xbox 360, aux clones et au pont
        // Steam Controller : la famille reste Xbox 360, mais aucun modèle
        // officiel n'est inventé. Le nom matériel (ou la liaison) est affiché.
        (0x045e, 0x028e) => PadIdentity {
            brand: Some(Xbox),
            family: Some("Xbox 360"),
            model: None,
        },
        (0x045e, 0x02d1 | 0x02dd | 0x02e0 | 0x02ea | 0x02fd) => {
            known(Xbox, "Xbox One", "Xbox Wireless Controller")
        }
        (0x045e, 0x02e3 | 0x02ff) => known(Xbox, "Xbox One", "Xbox Elite Controller"),
        (0x045e, 0x0b00) => known(Xbox, "Xbox", "Xbox Elite Series 2"),
        (0x045e, 0x0b12 | 0x0b13) => known(Xbox, "Xbox Series", "Xbox Wireless Controller"),
        // Nintendo
        (0x057e, 0x2006) => known(Nintendo, "Switch", lapinux_core::i18n::t("pad.joycon_left")),
        (0x057e, 0x2007) => known(
            Nintendo,
            "Switch",
            lapinux_core::i18n::t("pad.joycon_right"),
        ),
        (0x057e, 0x2009) => known(Nintendo, "Switch", "Switch Pro Controller"),
        (0x057e, 0x0330) => known(Nintendo, "Wii U", "Wii U Pro Controller"),
        (0x057e, 0x0306) => known(Nintendo, "Wii", "Wii Remote"),
        // Valve
        (0x28de, 0x0476 | 0x1102 | 0x1142 | 0x11fc | 0x1201) => {
            known(Steam, "Steam", "Steam Controller")
        }
        // Steam Controller 2026 (« Triton ») : filaire 1302, Bluetooth LE
        // 1303, Puck 1304 et récepteur Nereid 1305. Le noyau la nomme « Steam
        // Controller » sur son nœud evdev, et « Wireless Steam Controller »
        // derrière le Puck — un nom que porte aussi la manette 2015 sans fil.
        // L'identifiant USB est donc la seule source fiable de génération.
        (vendor, product) if lapinux_core::model::is_steam_controller_2026(vendor, product) => {
            known(Steam, "Steam", "Steam Controller 2026")
        }
        (0x28de, 0x1205) => known(Steam, "Steam Deck", "Steam Deck"),
        _ => return None,
    };
    Some(identity)
}

fn from_name(raw: &str) -> Option<PadIdentity> {
    let name = raw.to_ascii_lowercase();
    let has = |needle: &str| name.contains(needle);
    let sony = |family: &'static str, model: Option<&'static str>| PadIdentity {
        brand: Some(Brand::PlayStation),
        family: Some(family),
        model,
    };

    // Sony : le plus précis d'abord.
    if has("dualsense edge") {
        return Some(sony("PS5", Some("DualSense Edge")));
    }
    if has("dualsense") {
        return Some(sony("PS5", Some("DualSense")));
    }
    if has("dualshock 4") || has("dualshock4") {
        return Some(sony("PS4", Some("DualShock 4")));
    }
    if has("dualshock 3") || has("dualshock3") {
        return Some(sony("PS3", Some("DualShock 3")));
    }
    if has("playstation classic") {
        return Some(sony("PS1", Some("PlayStation Classic")));
    }
    if has("playstation move") {
        return Some(sony("PS3", Some("PlayStation Move")));
    }
    if has("playstation 5") || has("ps5") {
        return Some(sony("PS5", None));
    }
    if has("playstation 4") || has("ps4") {
        return Some(sony("PS4", None));
    }
    if has("playstation 3") || has("ps3") {
        return Some(sony("PS3", None));
    }
    if has("playstation 2") || has("ps2") {
        return Some(sony("PS2", None));
    }
    if has("ps1") || has("playstation 1") {
        return Some(sony("PS1", None));
    }
    if has("playstation") {
        // « PlayStation » seul ne dit pas de quelle génération il s'agit.
        return Some(PadIdentity {
            brand: Some(Brand::PlayStation),
            family: None,
            model: None,
        });
    }

    // Nintendo : « Pro Controller » seul reste ambigu (clones génériques),
    // seules les variantes explicites sont reconnues.
    if has("switch pro") || (has("nintendo") && has("pro controller")) {
        return Some(known(Brand::Nintendo, "Switch", "Switch Pro Controller"));
    }
    if has("joy-con") || has("joycon") {
        let model = if has("(l)") || has("left") || has("gauche") {
            lapinux_core::i18n::t("pad.joycon_left")
        } else if has("(r)") || has("right") || has("droite") {
            lapinux_core::i18n::t("pad.joycon_right")
        } else {
            "Joy-Con"
        };
        return Some(known(Brand::Nintendo, "Switch", model));
    }
    if has("wii u pro") {
        return Some(known(Brand::Nintendo, "Wii U", "Wii U Pro Controller"));
    }
    if has("nintendo switch") || has("switch") {
        return Some(PadIdentity {
            brand: Some(Brand::Nintendo),
            family: Some("Switch"),
            model: None,
        });
    }
    if has("wii") {
        return Some(PadIdentity {
            brand: Some(Brand::Nintendo),
            family: Some("Wii"),
            model: None,
        });
    }

    // Xbox : la famille vient du nom, le modèle reste le nom officiel.
    if has("xbox series") {
        return Some(PadIdentity {
            brand: Some(Brand::Xbox),
            family: Some("Xbox Series"),
            model: None,
        });
    }
    if has("xbox one") {
        return Some(PadIdentity {
            brand: Some(Brand::Xbox),
            family: Some("Xbox One"),
            model: None,
        });
    }
    if has("xbox 360") {
        return Some(PadIdentity {
            brand: Some(Brand::Xbox),
            family: Some("Xbox 360"),
            model: None,
        });
    }
    if has("xbox") {
        return Some(PadIdentity {
            brand: Some(Brand::Xbox),
            family: None,
            model: None,
        });
    }

    if has("steam deck") {
        return Some(known(Brand::Steam, "Steam Deck", "Steam Deck"));
    }
    if has("steam controller") {
        return Some(known(Brand::Steam, "Steam", "Steam Controller"));
    }
    if has("steam") {
        return Some(PadIdentity {
            brand: Some(Brand::Steam),
            family: None,
            model: None,
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playstation_ids_give_brand_family_and_model() {
        let dualsense = identify(0x054c, 0x0ce6, "DualSense Wireless Controller");
        assert_eq!(dualsense.brand, Some(Brand::PlayStation));
        assert_eq!(dualsense.family, Some("PS5"));
        assert_eq!(
            dualsense.display_name("DualSense Wireless Controller"),
            "DualSense"
        );

        assert_eq!(identify(0x054c, 0x0df2, "").model, Some("DualSense Edge"));
        let ds4 = identify(0x054c, 0x09cc, "Wireless Controller");
        assert_eq!(ds4.family, Some("PS4"));
        assert_eq!(ds4.display_name("Wireless Controller"), "DualShock 4");
        assert_eq!(identify(0x054c, 0x0268, "").family, Some("PS3"));
        assert_eq!(identify(0x054c, 0x0cda, "").family, Some("PS1"));
    }

    #[test]
    fn switch_pro_is_renamed_even_when_evdev_calls_it_pro_controller() {
        let pro = identify(0x057e, 0x2009, "Pro Controller");
        assert_eq!(pro.brand, Some(Brand::Nintendo));
        assert_eq!(pro.family, Some("Switch"));
        assert_eq!(pro.display_name("Pro Controller"), "Switch Pro Controller");

        assert_eq!(
            identify(0x057e, 0x2006, "").model,
            Some(lapinux_core::i18n::t("pad.joycon_left"))
        );
        assert_eq!(
            identify(0x057e, 0x2007, "").model,
            Some(lapinux_core::i18n::t("pad.joycon_right"))
        );
        assert_eq!(identify(0x057e, 0x0330, "").family, Some("Wii U"));
    }

    #[test]
    fn xbox_and_steam_ids_are_recognised() {
        let series = identify(0x045e, 0x0b12, "Xbox Wireless Controller");
        assert_eq!(series.brand, Some(Brand::Xbox));
        assert_eq!(series.family, Some("Xbox Series"));

        assert_eq!(identify(0x045e, 0x028e, "").family, Some("Xbox 360"));
        // Un clone 045e:028e ne revendique ni console ni modèle officiel :
        // son nom vient de la liaison, pas d'un « Xbox 360 Controller ».
        let clone = identify(0x045e, 0x028e, lapinux_core::i18n::t("pad.radio_2_4ghz"));
        assert_eq!(clone.brand, Some(Brand::Generic));
        assert_eq!(clone.family, None);
        assert_eq!(clone.model, None);
        assert_eq!(
            clone.display_name(lapinux_core::i18n::t("pad.radio_2_4ghz")),
            lapinux_core::i18n::t("pad.radio_2_4ghz")
        );
        assert_eq!(identify(0x28de, 0x1142, "").model, Some("Steam Controller"));
        assert_eq!(identify(0x28de, 0x1205, "").family, Some("Steam Deck"));

        // Steam Controller 2026 : chaque liaison garde le même modèle, quel
        // que soit le nom annoncé par evdev.
        assert_eq!(
            identify(0x28de, 0x1302, "Steam Controller").model,
            Some("Steam Controller 2026")
        );
        assert_eq!(
            identify(0x28de, 0x1304, "Wireless Steam Controller").model,
            Some("Steam Controller 2026")
        );
        assert_eq!(
            identify(0x28de, 0x1303, "").model,
            Some("Steam Controller 2026")
        );
        assert_eq!(
            identify(0x28de, 0x1305, "").model,
            Some("Steam Controller 2026")
        );
        // Le nom « Wireless Steam Controller » ne suffit pas à lui seul : la
        // manette 2015 sans fil le porte aussi.
        assert_eq!(
            identify(0x28de, 0x1142, "Wireless Steam Controller").model,
            Some("Steam Controller")
        );
    }

    #[test]
    fn unknown_ids_fall_back_to_the_name() {
        let ps2 = identify(0x0810, 0x0001, "PS2 Controller");
        assert_eq!(ps2.brand, Some(Brand::PlayStation));
        assert_eq!(ps2.family, Some("PS2"));

        let xbox = identify(0, 0, "Xbox 360 Controller for Windows");
        assert_eq!(xbox.family, Some("Xbox 360"));

        let switch = identify(0, 0, "Nintendo Switch Pro Controller");
        assert_eq!(switch.model, Some("Switch Pro Controller"));

        // Une manette d'adaptateur dont le nom ne précise pas la génération
        // garde la marque mais n'invente pas « PS1 ».
        let generic = identify(0, 0, "PlayStation Controller");
        assert_eq!(generic.brand, Some(Brand::PlayStation));
        assert_eq!(generic.family, None);
    }

    #[test]
    fn unknown_pads_are_named_inconnu_without_losing_their_name() {
        let pad = identify(0x1234, 0x5678, "Zebra Pad");
        assert_eq!(pad.brand, Some(Brand::Generic));
        assert_eq!(pad.family, None);
        assert_eq!(pad.display_name("Zebra Pad"), "Zebra Pad");

        // « Pro Controller » seul ne suffit pas à revendiquer Nintendo :
        // la marque et la console restent inconnues, mais le nom est conservé.
        let pro = identify(0, 0, "Pro Controller");
        assert_eq!(pro.brand, Some(Brand::Generic));
        assert_eq!(pro.family, None);
        assert_eq!(pro.display_name("Pro Controller"), "Pro Controller");
    }

    #[test]
    fn the_bridge_pad_is_named_as_a_steam_controller() {
        // Le pont expose la Steam Controller 2026 en Xbox 360 uinput ; la
        // fiche ne doit pas reprendre cette identité trompeuse.
        let identity = steam_bridge_identity();
        assert_eq!(identity.brand, Some(Brand::Steam));
        assert_eq!(identity.family, Some("Steam"));
        assert_eq!(
            identity.display_name("Microsoft X-Box 360 pad"),
            "Steam Controller 2026"
        );
    }
}
