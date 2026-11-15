//! Conversion d'un profil Steam Input (VDF) en profil de manette Lapinux.
//!
//! Steam applique les groupes d'un `controller_mappings` à la manette ; Lapinux
//! convertit les remaps de boutons qui ont une sortie Xbox. Le pilote natif
//! et le pont historique ont des codes de palettes distincts ; les axes,
//! pavés et actions avancées ne sont pas convertis par ce parseur.
//!
//! Le parseur VDF général ne garde qu'une clé `group` : les profils Steam en
//! contiennent plusieurs. On extrait donc chaque bloc de premier niveau avant
//! de le confier au parseur, ce qui conserve tous les groupes et les presets.

use crate::vdf::{self, Obj};
use lapinux_input::codes::*;
use lapinux_input::Profile;
use std::collections::HashMap;
use std::path::Path;

/// Charge un profil Steam Input et le convertit pour [`InputService`].
///
/// Un fichier absent, illisible ou sans `controller_mappings` ne doit pas
/// empêcher le lancement : l'appelant garde alors son profil TOML.
pub fn load(path: &Path) -> Option<Profile> {
    let text = std::fs::read_to_string(path).ok()?;
    from_vdf(&text)
}

/// Convertit le texte d'un profil Steam Input.
///
/// Le preset `Default` (le jeu) est prioritaire. Le preset `Menu` de Steam est
/// un jeu d'actions séparé que Steam choisit lui-même ; sans le client Steam,
/// Lapinux applique le preset de jeu. Les groupes de pavés tactiles sont
/// ignorés : leur conversion vers des actions Steam n'est pas implémentée.
pub fn from_vdf(text: &str) -> Option<Profile> {
    let root = vdf::parse_root(text).ok()?;
    let mapping = root.obj("controller_mappings").unwrap_or(&root);
    let title = mapping
        .str("title")
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or("Steam Input");

    let presets = blocks(text, "preset")
        .into_iter()
        .filter_map(|block| parse_block(block, "preset"))
        .collect::<Vec<_>>();
    let preset = presets
        .iter()
        .find(|preset| preset.str("name") == Some("Default"))
        .or_else(|| presets.first())?;
    let bindings = preset.obj("group_source_bindings")?;
    let selected: HashMap<&str, &str> = bindings.strings().collect();

    let mut profile = Profile::default();
    profile.name = title.to_string();
    for block in blocks(text, "group") {
        let Some(group) = parse_block(block, "group") else {
            continue;
        };
        let Some(id) = group.str("id") else {
            continue;
        };
        let Some(source) = selected.get(id) else {
            continue;
        };
        add_group(&mut profile, &group, source);
    }
    Some(profile)
}

/// Ajoute les remaps de boutons d'un groupe Steam au profil Lapinux.
fn add_group(profile: &mut Profile, group: &Obj, source: &str) {
    let source = source.split_whitespace().next().unwrap_or_default();
    // Un clic ou un geste de pavé exige la sémantique du groupe Steam ;
    // on ne prétend pas convertir ces actions en simple bouton Xbox.
    if source.contains("trackpad") {
        return;
    }
    let Some(inputs) = group.obj("inputs") else {
        return;
    };
    for (input_name, input) in inputs.objects() {
        let Some(source_code) = input_code(input_name, source) else {
            continue;
        };
        let Some(target_code) = first_binding(input) else {
            continue;
        };
        if source_code == target_code {
            continue;
        }
        let (Some(source_name), Some(target_name)) =
            (button_name(source_code), button_name(target_code))
        else {
            continue;
        };
        profile
            .remap
            .insert(source_name.to_string(), target_name.to_string());
        // hid-steam nomme explicitement les palettes hautes GRIP et les
        // basses GRIP2. Conserver séparément les codes du pont historique.
        let native = match input_name {
            "button_back_left_upper" => Some(BTN_GRIPL),
            "button_back_right_upper" => Some(BTN_GRIPR),
            "button_back_left" => Some(BTN_GRIPL2),
            "button_back_right" => Some(BTN_GRIPR2),
            _ => None,
        };
        if let Some(native) = native.and_then(button_name) {
            profile
                .remap
                .insert(native.to_string(), target_name.to_string());
        }
    }
}

/// Nom evdev de l'entrée Steam selon le groupe qui la déclare.
fn input_code(name: &str, source: &str) -> Option<u16> {
    match name {
        "button_a" => Some(BTN_SOUTH),
        "button_b" => Some(BTN_EAST),
        "button_x" => Some(BTN_WEST),
        "button_y" => Some(BTN_NORTH),
        "button_escape" => Some(BTN_START),
        "button_menu" => Some(BTN_SELECT),
        "left_bumper" => Some(BTN_TL),
        "right_bumper" => Some(BTN_TR),
        // Le pont expose les palettes dans l'ordre R4, L4, R5, L5, QAM :
        // HAPPY1 = R4, HAPPY2 = L4, HAPPY3 = R5, HAPPY4 = L5.
        "button_back_left" => Some(BTN_TRIGGER_HAPPY2),
        "button_back_right" => Some(BTN_TRIGGER_HAPPY1),
        "button_back_left_upper" => Some(BTN_TRIGGER_HAPPY4),
        "button_back_right_upper" => Some(BTN_TRIGGER_HAPPY3),
        // Le clic de stick est un bouton du groupe joystick ; son nom Steam
        // reste "click" pour les deux sticks.
        "click" if source == "joystick" => Some(BTN_THUMBL),
        "click" if source == "right_joystick" => Some(BTN_THUMBR),
        // Le chapeau directionnel est déjà émis par le pad physique en ABS_
        // HAT0X/Y ; le profil TOML ne sait pas remplacer un axe par un bouton.
        _ => None,
    }
}

/// Premier binding exploitable d'une entrée Steam (`Full_Press`, `Soft_Press`…).
fn first_binding(input: &Obj) -> Option<u16> {
    let activators = input.obj("activators")?;
    for (_, activator) in activators.objects() {
        let Some(bindings) = activator.obj("bindings") else {
            continue;
        };
        let Some(binding) = bindings.str("binding") else {
            continue;
        };
        if let Some(code) = output_code(binding) {
            return Some(code);
        }
    }
    None
}

/// Code virtuel d'une sortie `xinput_button` Steam.
fn output_code(binding: &str) -> Option<u16> {
    let rest = binding.trim().strip_prefix("xinput_button")?;
    let name = rest.split(',').next()?.trim().to_ascii_lowercase();
    match name.as_str() {
        "a" => Some(BTN_SOUTH),
        "b" => Some(BTN_EAST),
        "x" => Some(BTN_WEST),
        "y" => Some(BTN_NORTH),
        "start" => Some(BTN_START),
        "select" => Some(BTN_SELECT),
        "shoulder_left" => Some(BTN_TL),
        "shoulder_right" => Some(BTN_TR),
        "joystick_left" => Some(BTN_THUMBL),
        "joystick_right" => Some(BTN_THUMBR),
        _ => None,
    }
}

/// Blocs de premier niveau d'une clé, avec leurs accolades, dans l'ordre du
/// fichier. Le document Steam a plusieurs clés `group` identiques ; on ne peut
/// donc pas passer par un objet à clés uniques pour les récupérer.
fn blocks<'a>(text: &'a str, wanted: &str) -> Vec<&'a str> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    let mut depth = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'"' {
            match bytes[index] {
                b'{' => depth += 1,
                b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
            index += 1;
            continue;
        }
        let Some((key, end)) = quoted_key(bytes, index) else {
            break;
        };
        let brace = skip_ws(bytes, end);
        if depth == 1 && bytes.get(brace) == Some(&b'{') {
            let Some(close) = matching_brace(bytes, brace) else {
                break;
            };
            if key == wanted {
                out.push(&text[index..=close]);
            }
            // Le bloc entier est déjà traité : ne pas parcourir ses clés
            // internes, qui pourraient aussi s'appeler "group".
            index = close + 1;
            continue;
        }
        index = end;
    }
    out
}

fn parse_block(block: &str, key: &str) -> Option<Obj> {
    vdf::parse(block).ok()?.obj(key).cloned()
}

/// Lit une clé entre guillemets et renvoie sa fin (après le guillemet).
fn quoted_key(bytes: &[u8], start: usize) -> Option<(String, usize)> {
    let mut index = start + 1;
    let mut escaped = false;
    let mut end = None;
    while index < bytes.len() {
        let byte = bytes[index];
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            end = Some(index);
            break;
        }
        index += 1;
    }
    let end = end?;
    let key = String::from_utf8_lossy(&bytes[start + 1..end]).into_owned();
    Some((key, end + 1))
}

fn skip_ws(bytes: &[u8], mut index: usize) -> usize {
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    index
}

/// Indice de l'accolade fermant le bloc ouvert à `start`.
fn matching_brace(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut quote = false;
    let mut escaped = false;
    for (offset, byte) in bytes[start..].iter().enumerate() {
        if quote {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                quote = false;
            }
            continue;
        }
        match *byte {
            b'"' => quote = true,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(start + offset);
                }
            }
            _ => {}
        }
    }
    None
}

/// Détail lisible d'un profil Steam Input : le preset appliqué, ses groupes
/// d'actions et les réglages du profil. Les valeurs restent celles du VDF ;
/// l'interface les présente sans en inventer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProfileDetails {
    pub preset: String,
    pub groups: Vec<lapinux_core::model::SteamInputActionGroup>,
    pub settings: Vec<lapinux_core::model::SteamInputSetting>,
}

/// Extrait les actions et réglages d'un profil Steam Input.
///
/// Seuls les groupes sélectionnés par le preset `Default` sont retenus : Steam
/// peut embarquer des groupes de menu ou d'autres configurations dans le même
/// fichier, et les afficher tous donnerait une vue fausse de ce que le jeu
/// recevra. Un fichier sans preset reste lisible : ses groupes sont alors tous
/// exposés, faute d'information pour en choisir un.
pub fn profile_details(text: &str) -> Result<ProfileDetails, String> {
    use lapinux_core::model::{SteamInputAction, SteamInputActionGroup};

    let root = vdf::parse_root(text).map_err(|error| error.to_string())?;
    let mapping = root.obj("controller_mappings").unwrap_or(&root);
    let presets = blocks(text, "preset")
        .into_iter()
        .filter_map(|block| parse_block(block, "preset"))
        .collect::<Vec<_>>();
    let preset = presets
        .iter()
        .find(|preset| preset.str("name") == Some("Default"))
        .or_else(|| presets.first());
    let selected: HashMap<String, String> = preset
        .and_then(|preset| preset.obj("group_source_bindings"))
        .map(|bindings| {
            bindings
                .strings()
                .map(|(id, source)| (id.to_string(), source.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let mut groups = Vec::new();
    for block in blocks(text, "group") {
        let Some(group) = parse_block(block, "group") else {
            continue;
        };
        let id = group.str("id").unwrap_or("").trim();
        if !selected.is_empty() && !selected.contains_key(id) {
            continue;
        }
        let source = selected
            .get(id)
            .map(|source| source.split_whitespace().next().unwrap_or(""))
            .unwrap_or("")
            .to_string();
        let mut actions = Vec::new();
        if let Some(inputs) = group.obj("inputs") {
            for (input_name, input) in inputs.objects() {
                let before = actions.len();
                if let Some(activators) = input.obj("activators") {
                    for (activator_name, activator) in activators.objects() {
                        collect_bindings(&mut actions, input_name, activator_name, activator);
                    }
                }
                // Certains profils récents déclarent directement le binding
                // sous l'entrée, sans bloc d'activateur.
                if actions.len() == before {
                    if let Some(binding) =
                        input.str("binding").filter(|text| !text.trim().is_empty())
                    {
                        actions.push(SteamInputAction {
                            input: input_name.to_string(),
                            activator: String::new(),
                            binding: binding.trim().to_string(),
                        });
                    }
                }
            }
        }
        let settings = settings_of(group.obj("settings"));
        if actions.is_empty() && settings.is_empty() {
            continue;
        }
        groups.push(SteamInputActionGroup {
            source,
            mode: group.str("mode").unwrap_or("").trim().to_string(),
            actions,
            settings,
        });
    }

    Ok(ProfileDetails {
        preset: preset
            .and_then(|preset| preset.str("name"))
            .unwrap_or("")
            .trim()
            .to_string(),
        groups,
        settings: settings_of(mapping.obj("settings")),
    })
}

fn collect_bindings(
    out: &mut Vec<lapinux_core::model::SteamInputAction>,
    input: &str,
    activator: &str,
    object: &Obj,
) {
    use lapinux_core::model::SteamInputAction;
    let mut push = |binding: &str| {
        let binding = binding.trim();
        if !binding.is_empty() {
            out.push(SteamInputAction {
                input: input.to_string(),
                activator: activator.to_string(),
                binding: binding.to_string(),
            });
        }
    };
    if let Some(bindings) = object.obj("bindings") {
        for (_, value) in &bindings.0 {
            match value {
                vdf::Value::Str(binding) => push(binding),
                vdf::Value::Obj(inner) => {
                    if let Some(binding) = inner.str("binding") {
                        push(binding);
                    }
                }
            }
        }
    }
    if let Some(binding) = object.str("binding") {
        push(binding);
    }
}

fn settings_of(object: Option<&Obj>) -> Vec<lapinux_core::model::SteamInputSetting> {
    use lapinux_core::model::SteamInputSetting;
    let Some(object) = object else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, value) in &object.0 {
        match value {
            vdf::Value::Str(value) if !value.trim().is_empty() => {
                out.push(SteamInputSetting {
                    key: key.clone(),
                    value: value.trim().to_string(),
                });
            }
            vdf::Value::Obj(inner) => {
                for (sub_key, sub_value) in &inner.0 {
                    if let vdf::Value::Str(value) = sub_value {
                        if !value.trim().is_empty() {
                            out.push(SteamInputSetting {
                                key: format!("{key}.{sub_key}"),
                                value: value.trim().to_string(),
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile_text() -> String {
        r#"
        "controller_mappings"
        {
            "title" "MOTORSLICE test"
            "preset"
            {
                "id" "0"
                "name" "Default"
                "group_source_bindings"
                {
                    "0" "button_diamond active"
                    "7" "switch active"
                    "18" "right_trackpad active"
                }
            }
            "group"
            {
                "id" "0"
                "mode" "four_buttons"
                "inputs"
                {
                    "button_a" { "activators" { "Full_Press" { "bindings" { "binding" "xinput_button B, , " } } } }
                }
            }
            "group"
            {
                "id" "7"
                "mode" "switches"
                "inputs"
                {
                    "button_back_left" { "activators" { "Full_Press" { "bindings" { "binding" "xinput_button A, Jump, " } } } }
                    "button_back_right" { "activators" { "Full_Press" { "bindings" { "binding" "xinput_button SHOULDER_RIGHT, , " } } } }
                    "button_back_left_upper" { "activators" { "Full_Press" { "bindings" { "binding" "xinput_button X, , " } } } }
                    "button_back_right_upper" { "activators" { "Full_Press" { "bindings" { "binding" "xinput_button B, , " } } } }
                }
            }
            "group"
            {
                "id" "18"
                "mode" "absolute_mouse"
                "inputs"
                {
                    "click" { "activators" { "Soft_Press" { "bindings" { "binding" "mouse_button LEFT, , " } } } }
                }
            }
            "group"
            {
                "id" "9"
                "mode" "dpad"
                "inputs"
                {
                    "dpad_north" { "activators" { "Full_Press" { "bindings" { "binding" "key_press UP, , " } } } }
                }
            }
            "settings" { "left_trackpad_mode" "0" "left_stick_deadzone" "0.15" }
        }
        "#
        .to_string()
    }

    #[test]
    fn steam_groups_and_back_buttons_are_translated() {
        let profile = from_vdf(&profile_text()).expect("profil converti");
        assert_eq!(profile.name, "MOTORSLICE test");
        assert_eq!(
            profile.remap.get("BTN_SOUTH").map(String::as_str),
            Some("BTN_EAST")
        );
        assert_eq!(
            profile.remap.get("BTN_TRIGGER_HAPPY1").map(String::as_str),
            Some("BTN_TR")
        );
        assert_eq!(
            profile.remap.get("BTN_TRIGGER_HAPPY2").map(String::as_str),
            Some("BTN_SOUTH")
        );
        assert_eq!(
            profile.remap.get("BTN_TRIGGER_HAPPY3").map(String::as_str),
            Some("BTN_EAST")
        );
        assert_eq!(
            profile.remap.get("BTN_TRIGGER_HAPPY4").map(String::as_str),
            Some("BTN_WEST")
        );
        assert_eq!(
            profile.remap.get("BTN_GRIPL").map(String::as_str),
            Some("BTN_WEST")
        );
        assert_eq!(
            profile.remap.get("BTN_GRIPR").map(String::as_str),
            Some("BTN_EAST")
        );
        assert_eq!(
            profile.remap.get("BTN_GRIPL2").map(String::as_str),
            Some("BTN_SOUTH")
        );
        assert_eq!(
            profile.remap.get("BTN_GRIPR2").map(String::as_str),
            Some("BTN_TR")
        );
    }

    #[test]
    fn le_detail_garde_les_groupes_du_preset_et_leurs_reglages() {
        let details = profile_details(&profile_text()).expect("détail lu");
        assert_eq!(details.preset, "Default");
        assert_eq!(
            details.groups.len(),
            3,
            "le groupe dpad hors preset est ignoré"
        );
        let buttons = details
            .groups
            .iter()
            .find(|group| group.source == "button_diamond")
            .expect("groupe des boutons");
        assert_eq!(buttons.mode, "four_buttons");
        assert!(buttons.actions.iter().any(|action| {
            action.input == "button_a"
                && action.activator == "Full_Press"
                && action.binding.contains("xinput_button B")
        }));
        let trackpad = details
            .groups
            .iter()
            .find(|group| group.source == "right_trackpad")
            .expect("groupe du pavé");
        assert_eq!(trackpad.mode, "absolute_mouse");
        assert!(trackpad
            .actions
            .iter()
            .any(|action| action.binding.contains("mouse_button LEFT")));
        assert!(details
            .settings
            .iter()
            .any(|setting| setting.key == "left_stick_deadzone" && setting.value == "0.15"));
    }

    #[test]
    fn un_profil_partiel_reste_lisible_sans_inventer_d_actions() {
        let details = profile_details(
            "\"controller_mappings\"\n{\n\t\"title\" \"Partiel\"\n\t\"group\"\n\t{\n\t\t\"id\" \"0\"\n\t}\n}\n",
        )
        .expect("le détail partiel doit rester lisible");
        assert_eq!(details.preset, "");
        assert!(details.groups.is_empty());
        assert!(details.settings.is_empty());
        assert!(profile_details("ceci n'est pas un VDF").is_err());
    }

    #[test]
    fn trackpad_groups_do_not_create_mouse_or_invented_buttons() {
        let profile = from_vdf(&profile_text()).expect("profil converti");
        assert!(!profile
            .remap
            .values()
            .any(|target| target.contains("mouse")));
        assert!(!profile.remap.contains_key("ABS_MT_POSITION_X"));
    }
}
