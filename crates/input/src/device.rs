//! Énumération des manettes exposées par evdev.

use crate::codes::*;
use lapinux_core::model::{DeviceInfo, DeviceLink, UdevIdentity};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// Récepteurs et adaptateurs USB connus qui exposent la manette comme un
/// périphérique USB alors qu'elle communique sans fil avec un dongle. Cette
/// liste n'est qu'un raccourci : les chaînes produit/fabricant restent la
/// source de vérité pour les modèles absents.
const USB_RECEIVERS: [(u16, u16); 22] = [
    (0x28de, 0x1304), // Steam Controller Puck
    (0x28de, 0x1305), // Steam Controller Puck (variante)
    (0x28de, 0x1142), // récepteur Steam Controller
    (0x045e, 0x02e6), // adaptateur Xbox sans fil
    (0x045e, 0x02fe), // adaptateur Xbox sans fil (modèle suivant)
    (0x045e, 0x0719), // récepteur Xbox 360 sans fil
    (0x045e, 0x0291), // récepteur Xbox 360 sans fil (non officiel)
    (0x045e, 0x02a0), // récepteur Xbox 360 Big Button
    (0x046d, 0xc21f), // récepteur Logitech F710
    (0x046d, 0xc219), // récepteur Logitech sans fil
    (0x054c, 0x0ba0), // récepteur USB DualShock 4
    (0x2dc8, 0x3106), // récepteur 2,4 GHz 8BitDo Ultimate
    (0x2dc8, 0x3016), // adaptateur USB 8BitDo
    (0x0079, 0x1843), // adaptateur GameCube Mayflash
    (0x0079, 0x1844), // adaptateur GameCube Mayflash (variante)
    (0x0079, 0x1800), // adaptateur Wii U Pro Mayflash
    (0x0079, 0x18d2), // adaptateur Magic-NS Mayflash
    (0x0079, 0x18d3), // adaptateur Magic-NS Mayflash (variante)
    (0x0c12, 0x07f4), // convertisseur NeoGeo Brook
    (0x0c12, 0x08f1), // convertisseur PS2 Brook
    (0x0e6f, 0x0003), // récepteur sans fil Pelican Xbox
    (0x0e6f, 0x0005), // récepteur sans fil Pelican Eclipse
];

pub fn is_gamepad(device: &evdev::Device) -> bool {
    let has_pad_button = device
        .supported_keys()
        .map(|keys| {
            keys.contains(evdev::KeyCode::new(BTN_SOUTH))
        })
        .unwrap_or(false);
    let has_stick = device
        .supported_absolute_axes()
        .map(|axes| {
            axes.contains(evdev::AbsoluteAxisCode(ABS_X))
                && axes.contains(evdev::AbsoluteAxisCode(ABS_Y))
        })
        .unwrap_or(false);
    has_pad_button && has_stick
}

pub fn axes_of(device: &evdev::Device) -> Vec<String> {
    device
        .supported_absolute_axes()
        .map(|axes| {
            axes.iter()
                .filter_map(|axis| axis_name(axis.0).map(|n| n.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Nom de secours quand le pilote n'annonce aucun nom : la liaison donne
/// tout de même une carte lisible, sans inventer une marque.
pub(crate) fn fallback_pad_name(link: DeviceLink) -> &'static str {
    match link {
        DeviceLink::Usb => "Manette USB",
        DeviceLink::Bluetooth => "Manette Bluetooth",
        DeviceLink::Dongle => "Manette 2,4 GHz",
        DeviceLink::Virtual => "Manette virtuelle",
        DeviceLink::Unknown => "Manette",
    }
}

/// Nom affiché d'un périphérique : celui annoncé par le pilote quand il
/// existe, sinon la liaison pour une manette, ou un libellé générique pour un
/// périphérique qui n'en est pas une.
fn display_name(raw: Option<&str>, gamepad: bool, link: DeviceLink) -> String {
    raw.map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if gamepad {
                fallback_pad_name(link).to_string()
            } else {
                "périphérique inconnu".to_string()
            }
        })
}

/// Un pad 045e:028e dont le fabricant USB n'est pas Microsoft est un clone
/// ou un pont : le même identifiant sert aux manettes Xbox 360, aux récepteurs
/// génériques et à la Steam Controller pontée. On le nomme par sa liaison au
/// lieu de lui attribuer un modèle officiel qu'il n'est pas.
fn is_generic_xbox(vendor: u16, product: u16, manufacturer: Option<&str>) -> bool {
    if vendor != 0x045e || product != 0x028e {
        return false;
    }
    manufacturer
        .filter(|name| !name.trim().is_empty())
        .map_or(false, |name| !name.to_ascii_lowercase().contains("microsoft"))
}

pub fn describe(path: &PathBuf, device: &evdev::Device) -> DeviceInfo {
    let id = device.input_id();
    let detected = connection_of(path);
    let generic_xbox = is_generic_xbox(
        id.vendor(),
        id.product(),
        usb_manufacturer(path).as_deref(),
    );
    // Un clone 045e:028e se présente presque toujours par un récepteur
    // 2,4 GHz ; la liaison sert de nom plutôt qu'un « Xbox 360 Controller »
    // trompeur.
    let connection = if generic_xbox {
        DeviceLink::Dongle
    } else {
        detected
    };
    let gamepad = is_gamepad(device);
    let name = if generic_xbox {
        fallback_pad_name(connection).to_string()
    } else {
        display_name(device.name(), gamepad, connection)
    };
    // L'identifiant HID est le même en USB et en Bluetooth sur les manettes
    // Sony : il porte le renommage et la sélection d'une liaison à l'autre.
    // Les autres périphériques (dongles, claviers) n'ont pas de MAC et
    // gardent `None`.
    let address = sysfs_dir(path)
        .and_then(|dir| unique_id(&dir))
        .filter(|id| is_mac(id));
    let battery_supply = battery_supply(path);
    let keys = device.supported_keys();
    let button_names: Vec<String> = keys
        .map(|keys| {
            keys.iter()
                .filter(|key| is_pad_button(key.0))
                .map(|key| button_label(key.0))
                .filter(|label| label != "?")
                .collect()
        })
        .unwrap_or_default();
    let buttons = button_names.len();
    let touchpad = device.supported_absolute_axes().map_or(false, |axes| {
        axes.contains(evdev::AbsoluteAxisCode(ABS_MT_POSITION_X))
            && axes.contains(evdev::AbsoluteAxisCode(ABS_MT_POSITION_Y))
    });
    DeviceInfo {
        path: path.clone(),
        name,
        vendor: id.vendor(),
        product: id.product(),
        gamepad,
        buttons,
        button_names,
        axes: axes_of(device),
        force_feedback: device
            .supported_ff()
            .map_or(false, |ff| ff.iter().next().is_some()),
        touchpad,
        connection,
        battery: battery_supply.as_ref().and_then(|dir| read_capacity(dir)),
        battery_status: battery_supply.as_ref().and_then(|dir| read_status(dir)),
        address,
        connected: true,
        steam_bridge: false,
        udev: udev_identity(path),
    }
}

/// Bus de la manette : le noyau l'indique dans `HID_ID` (0003 = USB,
/// 0005 = Bluetooth). Les périphériques non-HID n'ont pas cette ligne, on
/// retombe alors sur le chemin sysfs.
fn connection_of(event: &Path) -> DeviceLink {
    let Some(dir) = sysfs_dir(event) else {
        return DeviceLink::Unknown;
    };
    for ancestor in dir.ancestors().take(6) {
        if let Ok(uevent) = std::fs::read_to_string(ancestor.join("uevent")) {
            if let Some(line) = uevent.lines().find_map(|line| line.strip_prefix("HID_ID=")) {
                return match parse_hid_bus(line) {
                    DeviceLink::Usb => usb_link(&dir),
                    link => link,
                };
            }
        }
    }
    let path = dir.to_string_lossy();
    if path.contains("virtual/input") {
        DeviceLink::Virtual
    } else if path.contains("/usb") {
        usb_link(&dir)
    } else if path.contains("bluetooth") {
        DeviceLink::Bluetooth
    } else {
        DeviceLink::Unknown
    }
}

/// Distingue un câble d'un dongle en remontant jusqu'au périphérique USB et
/// en lisant ses chaînes produit/fabricant.
fn usb_link(dir: &Path) -> DeviceLink {
    for ancestor in dir.ancestors().take(8) {
        let Some(vendor) = read_hex(ancestor.join("idVendor")) else {
            continue;
        };
        let Some(product) = read_hex(ancestor.join("idProduct")) else {
            continue;
        };
        let product_name = read_text(ancestor.join("product"));
        let manufacturer = read_text(ancestor.join("manufacturer"));
        return if is_usb_receiver(vendor, product, &product_name, &manufacturer) {
            DeviceLink::Dongle
        } else {
            DeviceLink::Usb
        };
    }
    DeviceLink::Usb
}

fn is_usb_receiver(vendor: u16, product: u16, product_name: &str, manufacturer: &str) -> bool {
    if USB_RECEIVERS.contains(&(vendor, product)) {
        return true;
    }
    let haystack = format!("{product_name} {manufacturer}").to_lowercase();
    [
        "receiver",
        "dongle",
        "puck",
        "wireless adapter",
        "wireless usb adapter",
        "2.4g",
        "2.4 ghz",
    ]
    .iter()
    .any(|needle| haystack.contains(needle))
}

/// Chaîne `manufacturer` du périphérique USB parent, quand elle existe.
fn usb_manufacturer(path: &Path) -> Option<String> {
    let dir = sysfs_dir(path)?;
    for ancestor in dir.ancestors().take(8) {
        let text = read_text(ancestor.join("manufacturer"));
        if !text.is_empty() {
            return Some(text);
        }
    }
    None
}

fn read_hex(path: PathBuf) -> Option<u16> {
    u16::from_str_radix(read_text(path).trim_start_matches("0x"), 16).ok()
}

fn read_text(path: PathBuf) -> String {
    std::fs::read_to_string(path)
        .map(|text| text.trim().to_string())
        .unwrap_or_default()
}

/// Lit les noms matériels dans la base udev du nœud evdev. C'est la seule
/// source qui distingue deux clones partageant `045e:028e` : elle contient les
/// chaînes USB réelles (`Homertech`, `MICREAL GAMEPAD…`) et le numéro de série.
fn udev_identity(event: &Path) -> UdevIdentity {
    let mut identity = UdevIdentity::default();
    let Some(name) = event.file_name().and_then(|name| name.to_str()) else {
        return identity;
    };
    let dev = read_text(PathBuf::from(format!("/sys/class/input/{name}/dev")));
    if let Some((major, minor)) = dev.trim().split_once(':') {
        let data = std::fs::read_to_string(format!("/run/udev/data/c{major}:{minor}"))
            .unwrap_or_default();
        let field = |key: &str| {
            data.lines()
                .find_map(|line| line.strip_prefix(key))
                .map(|value| decode_udev(value.trim()))
                .filter(|value| !value.is_empty())
        };
        identity.vendor = field("E:ID_VENDOR_ENC=").or_else(|| field("E:ID_VENDOR="));
        identity.model = field("E:ID_MODEL_ENC=").or_else(|| field("E:ID_MODEL="));
        identity.serial = field("E:ID_SERIAL=")
            .filter(|serial| !serial.eq_ignore_ascii_case("noserial"));
    }
    if identity.is_empty() {
        // Pas de base udev (conteneur, nœud fraîchement créé) : les mêmes
        // chaînes existent dans sysfs, un cran au-dessus du nœud input.
        if let Some(dir) = sysfs_dir(event) {
            for ancestor in dir.ancestors().take(8) {
                if ancestor.join("idVendor").is_file() {
                    let vendor = read_text(ancestor.join("manufacturer"));
                    let model = read_text(ancestor.join("product"));
                    let serial = read_text(ancestor.join("serial"));
                    identity.vendor = (!vendor.is_empty()).then_some(vendor);
                    identity.model = (!model.is_empty()).then_some(model);
                    identity.serial = (!serial.is_empty()).then_some(serial);
                    break;
                }
            }
        }
    }
    identity
}

/// udev échappe les espaces et caractères non ASCII en `\xNN` dans les
/// champs `*_ENC`.
fn decode_udev(text: &str) -> String {
    let mut out: Vec<u8> = Vec::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&'x') {
            chars.next();
            let high = chars.next();
            let low = chars.next();
            if let (Some(high), Some(low)) = (high, low) {
                if let Ok(byte) = u8::from_str_radix(&format!("{high}{low}"), 16) {
                    out.push(byte);
                    continue;
                }
                out.extend_from_slice(b"\\x");
                let mut buffer = [0u8; 4];
                out.extend_from_slice(high.encode_utf8(&mut buffer).as_bytes());
                let mut buffer = [0u8; 4];
                out.extend_from_slice(low.encode_utf8(&mut buffer).as_bytes());
                continue;
            }
        }
        let mut buffer = [0u8; 4];
        out.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_hid_bus(hid_id: &str) -> DeviceLink {
    match hid_id.split(':').next().unwrap_or_default() {
        "0003" => DeviceLink::Usb,
        "0005" => DeviceLink::Bluetooth,
        _ => DeviceLink::Unknown,
    }
}

fn is_mac(value: &str) -> bool {
    value.split(':').count() == 6
        && value
            .split(':')
            .all(|part| part.len() == 2 && part.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Batterie exposée soit sous le périphérique HID, soit dans
/// `/sys/class/power_supply` avec l'identifiant unique (adresse Bluetooth).
/// Le dossier est renvoyé pour lire à la fois `capacity` et `status`.
fn battery_supply(event: &Path) -> Option<PathBuf> {
    let dir = sysfs_dir(event)?;
    for ancestor in dir.ancestors().take(6) {
        if let Ok(entries) = std::fs::read_dir(ancestor.join("power_supply")) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.join("capacity").is_file() || path.join("capacity_level").is_file() {
                    return Some(path);
                }
            }
        }
    }
    let uniq = unique_id(&dir)?;
    for entry in std::fs::read_dir("/sys/class/power_supply").ok()?.flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy().contains(uniq.trim())
            && (path.join("capacity").is_file() || path.join("capacity_level").is_file())
        {
            return Some(path);
        }
    }
    None
}

fn read_capacity(dir: &Path) -> Option<u8> {
    let mut value = None;
    if let Ok(text) = std::fs::read_to_string(dir.join("capacity")) {
        value = parse_capacity(&text);
    }
    if value.is_none() {
        // Le pilote hid-nintendo n'expose que `capacity_level` (Empty, Low,
        // Normal, High, Full) : la jauge doit tout de même apparaître.
        if let Ok(text) = std::fs::read_to_string(dir.join("capacity_level")) {
            value = parse_capacity_level(&text);
        }
    }
    let value = value?;
    // hid-steam initialise sa power_supply à 100 % et 3000 mV en attendant le
    // premier rapport 0x04. Ce n'est pas une batterie pleine : après chaque
    // reconnexion, la jauge afficherait sinon un faux 100 % pendant que le
    // firmware n'a encore rien envoyé.
    if value == 100 && read_voltage_now(dir) == Some(3_000_000) {
        return None;
    }
    // « Full » décrit souvent le chargeur branché, pas la batterie : ne pas
    // afficher 100 % pendant la charge. Le relevé live prend le relais dès
    // que le rapport HID arrive.
    let charging = read_status(dir).as_deref() == Some("Charging");
    Some(if charging { value.min(99) } else { value })
}

fn read_voltage_now(dir: &Path) -> Option<u32> {
    std::fs::read_to_string(dir.join("voltage_now"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn parse_capacity_level(text: &str) -> Option<u8> {
    match text.trim().to_ascii_lowercase().as_str() {
        "full" => Some(100),
        "high" => Some(75),
        "normal" => Some(50),
        "low" => Some(25),
        "critical" => Some(5),
        _ => None,
    }
}

fn read_status(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("status")).ok()?;
    let status = text.trim();
    (!status.is_empty()).then(|| status.to_string())
}

fn parse_capacity(text: &str) -> Option<u8> {
    text.trim().parse::<u8>().ok().map(|value| value.min(100))
}

fn unique_id(dir: &Path) -> Option<String> {
    for ancestor in dir.ancestors().take(6) {
        let Ok(uevent) = std::fs::read_to_string(ancestor.join("uevent")) else {
            continue;
        };
        if let Some(uniq) = uevent.lines().find_map(|line| line.strip_prefix("HID_UNIQ=")) {
            return Some(uniq.to_string());
        }
    }
    None
}

/// `/sys/class/input/eventX/device`, résolu pour suivre les liens des
/// périphériques USB et Bluetooth.
fn sysfs_dir(event: &Path) -> Option<PathBuf> {
    let name = event.file_name()?.to_str()?;
    if !name.starts_with("event") {
        return None;
    }
    std::fs::canonicalize(format!("/sys/class/input/{name}/device")).ok()
}

/// Le Puck de la Steam Controller 2026 est un dongle USB 2,4 GHz. Jusqu'au
/// noyau 7.2, il n'expose que son clavier et sa souris ; `hid-steam` crée le
/// gamepad, les capteurs et la batterie depuis le noyau 7.3, et un pont
/// userspace peut prendre le relais avant.
pub fn steam_puck_present() -> bool {
    let Ok(entries) = std::fs::read_dir("/sys/bus/usb/devices") else {
        return false;
    };
    entries.flatten().any(|entry| {
        let dir = entry.path();
        is_steam_puck(
            read_hex(dir.join("idVendor")).unwrap_or(0),
            read_hex(dir.join("idProduct")).unwrap_or(0),
        )
    })
}

fn is_steam_puck(vendor: u16, product: u16) -> bool {
    vendor == 0x28de && matches!(product, 0x1304 | 0x1305)
}

const STEAM_LIZARD_PATH: &str = "/sys/module/hid_steam/parameters/lizard_mode";
const STEAM_LIZARD_CONFIG_PATH: &str = "/etc/modprobe.d/99-steam-controller.conf";

/// État du mode clavier/souris de `hid-steam`. `None` si le module ou son
/// paramètre n'est pas disponible ; les manettes 2026 restent alors dans le
/// mode par défaut du pilote.
pub fn steam_lizard_mode() -> Option<bool> {
    let text = std::fs::read_to_string(STEAM_LIZARD_PATH).ok()?;
    match text.trim() {
        "Y" | "1" => Some(true),
        "N" | "0" => Some(false),
        _ => None,
    }
}

/// Vrai si le processus peut écrire le paramètre. C'est le cas après
/// l'installation de `packaging/99-steam-controller.rules`, qui en donne
/// l'écriture au groupe `wheel` ; sinon le fichier reste root-only.
pub fn steam_lizard_writable() -> bool {
    std::fs::OpenOptions::new()
        .write(true)
        .open(STEAM_LIZARD_PATH)
        .is_ok()
}

/// Bascule le mode clavier/souris à chaud. La valeur est relue après
/// l'écriture pour confirmer que le pilote l'a réellement appliquée.
pub fn set_steam_lizard_mode(enabled: bool) -> Result<(), String> {
    use std::io::Write;

    let value = if enabled { "Y" } else { "N" };
    // Pas de `std::fs::write` ici : il ouvre avec O_TRUNC, inutile et fragile
    // sur un attribut sysfs. Une écriture simple suffit.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(STEAM_LIZARD_PATH)
        .map_err(|error| format!("ouverture de lizard_mode impossible : {error}"))?;
    file.write_all(value.as_bytes())
        .map_err(|error| format!("écriture de lizard_mode impossible : {error}"))?;
    match steam_lizard_mode() {
        Some(mode) if mode == enabled => Ok(()),
        Some(mode) => Err(format!(
            "le pilote est resté en mode {}",
            if mode { "clavier/souris" } else { "manette native" }
        )),
        None => Err("le pilote n'expose plus lizard_mode".to_string()),
    }
}

/// Exécute un script fixe via `pkexec` : le bureau demande l'autorisation,
/// puis le script tourne en root. Aucun argument utilisateur n'entre dans le
/// shell privilégié.
fn run_pkexec_script(script: &str) -> Result<(), String> {
    let pkexec = Path::new("/usr/bin/pkexec");
    if !pkexec.exists() {
        return Err("pkexec introuvable : installe polkit pour autoriser l'installation".to_string());
    }
    let output = std::process::Command::new(pkexec)
        .args(["/bin/sh", "-c", script])
        .output()
        .map_err(|error| format!("pkexec n'a pas pu démarrer : {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        return Err(if stderr.is_empty() {
            format!("installation refusée ({})", output.status)
        } else {
            format!("installation refusée : {stderr}")
        });
    }
    Ok(())
}

/// Installe la règle udev depuis Lapinux, sans terminal. `pkexec` ouvre la
/// demande d'autorisation du bureau, puis le script (fixe, embarqué dans le
/// binaire) écrit la règle et recharge udev. Aucun chemin fourni par
/// l'utilisateur n'entre dans le shell root.
pub fn install_steam_lizard_rule() -> Result<(), String> {
    if steam_lizard_writable() {
        return Ok(());
    }
    let rule = include_str!("../../../packaging/99-steam-controller.rules").trim_end();
    let script = format!(
        "set -eu\n/usr/bin/cat > /etc/udev/rules.d/99-steam-controller.rules <<'LAPINUX_STEAM_RULE'\n{rule}\nLAPINUX_STEAM_RULE\n/usr/bin/chmod 0644 /etc/udev/rules.d/99-steam-controller.rules\n/usr/bin/udevadm control --reload-rules\n/usr/bin/udevadm trigger --action=add --subsystem-match=input\n"
    );
    run_pkexec_script(&script)?;
    // `udevadm trigger` rend la main avant que le démon ait fini d'appliquer
    // la permission : on relit le paramètre pendant quelques secondes.
    for _ in 0..50 {
        if steam_lizard_writable() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("règle installée, mais lizard_mode reste inaccessible : vérifie que ton compte est dans le groupe wheel".to_string())
}

/// Vrai si la configuration qui force le mode natif au chargement du pilote
/// est déjà installée.
pub fn steam_lizard_config_installed() -> bool {
    std::fs::read_to_string(STEAM_LIZARD_CONFIG_PATH)
        .map(|text| {
            text.lines()
                .any(|line| line.trim() == "options hid_steam lizard_mode=0")
        })
        .unwrap_or(false)
}

/// Installe `99-steam-controller.conf` depuis Lapinux. Le fichier ne change
/// rien à la session courante : il sera lu au prochain chargement de
/// `hid_steam`, donc au prochain démarrage.
pub fn install_steam_lizard_config() -> Result<(), String> {
    if steam_lizard_config_installed() {
        return Ok(());
    }
    let config = include_str!("../../../packaging/99-steam-controller.conf").trim_end();
    let script = format!(
        "set -eu\n/usr/bin/cat > {STEAM_LIZARD_CONFIG_PATH} <<'LAPINUX_STEAM_CONFIG'\n{config}\nLAPINUX_STEAM_CONFIG\n/usr/bin/chmod 0644 {STEAM_LIZARD_CONFIG_PATH}\n"
    );
    run_pkexec_script(&script)?;
    if steam_lizard_config_installed() {
        Ok(())
    } else {
        Err("configuration écrite, mais l'option lizard_mode=0 est introuvable".to_string())
    }
}

/// Le pont userspace remplace le pilote noyau : tant qu'il tourne, la carte
/// d'aide ne doit pas inviter à l'installer. Le nom est tronqué à 15 octets
/// dans `/proc/<pid>/comm`.
pub fn steam_puck_bridge_running() -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name();
        if !name.to_string_lossy().chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
        std::fs::read_to_string(entry.path().join("comm"))
            .map(|comm| is_bridge_process_name(&comm))
            .unwrap_or(false)
    })
}

fn is_bridge_process_name(comm: &str) -> bool {
    comm.trim().starts_with("steam-puck-brid")
}

/// Pad virtuel du pont `steam-puck-bridge` : pour la Steam Controller 2026,
/// le pont crée un uinput `045e:028e` nommé « Microsoft X-Box 360 pad » dès
/// qu'il prend le hidraw du Puck. Un vrai pad Xbox 360 est branché en USB,
/// jamais virtuel ; sans Puck ni pont, un pad virtuel vient d'ailleurs
/// (Steam Input, xboxdrv…).
fn is_steam_bridge_pad(
    vendor: u16,
    product: u16,
    connection: DeviceLink,
    name: &str,
    bridge_running: bool,
) -> bool {
    bridge_running
        && vendor == 0x045e
        && product == 0x028e
        && connection == DeviceLink::Virtual
        && name.eq_ignore_ascii_case("Microsoft X-Box 360 pad")
}

pub fn list() -> Vec<DeviceInfo> {
    // Le pont n'existe que tant que le Puck Valve est branché : la vérification
    // ne coûte qu'une lecture sysfs et /proc par énumération (mise en cache).
    let bridge_running = steam_puck_present() && steam_puck_bridge_running();
    let mut out: Vec<DeviceInfo> = evdev::enumerate()
        .map(|(path, device)| {
            let mut info = describe(&path, &device);
            let bridged = is_steam_bridge_pad(
                info.vendor,
                info.product,
                info.connection,
                &info.name,
                bridge_running,
            );
            info.steam_bridge = bridged;
            info
        })
        .collect();
    out.sort_by(|a, b| b.gamepad.cmp(&a.gamepad).then(a.name.cmp(&b.name)));
    out
}

/// Signature bon marché de `/dev/input` : deux appels système qui changent dès
/// qu'une manette apparaît, disparaît ou change de nœud.
fn input_stamp() -> Option<(SystemTime, usize)> {
    let modified = std::fs::metadata("/dev/input").ok()?.modified().ok()?;
    Some((modified, std::fs::read_dir("/dev/input").ok()?.count()))
}

/// Une batterie ou une liaison peuvent changer sans que le dossier bouge : le
/// cache se relit quand même au bout de ce délai.
const LIST_TTL: Duration = Duration::from_secs(30);

/// Dernière énumération et la signature qui l'a produite.
static LIST_CACHE: Mutex<Option<(Option<(SystemTime, usize)>, Instant, Vec<DeviceInfo>)>> =
    Mutex::new(None);

/// Énumération réutilisée par les sondages répétés de l'interface.
///
/// Décrire chaque nœud evdev lit une vingtaine de fichiers sysfs : sur une
/// machine à trente nœuds, un relevé coûte une centaine de millisecondes.
/// L'interface le demandait toutes les trois secondes, soit près de 5 % d'un
/// cœur en permanence. La réponse est gardée tant que `/dev/input` n'a pas
/// bougé ; la relecture de sûreté rafraîchit la batterie et l'état des liaisons.
pub fn list_cached() -> Vec<DeviceInfo> {
    let stamp = input_stamp();
    if let Ok(cache) = LIST_CACHE.lock() {
        if let Some((cached, at, devices)) = cache.as_ref() {
            if *cached == stamp && at.elapsed() < LIST_TTL {
                let mut devices = devices.clone();
                // Le cache évite de relire tout sysfs, mais la batterie Steam
                // arrive quelques secondes après la reconnexion. La relire à
                // chaque demande évite de garder le placeholder ou une valeur
                // d'avant la déconnexion pendant trente secondes.
                for device in &mut devices {
                    refresh_steam_battery(device);
                }
                return devices;
            }
        }
    }
    let devices = list();
    if let Ok(mut cache) = LIST_CACHE.lock() {
        *cache = Some((stamp, Instant::now(), devices.clone()));
    }
    devices
}

/// La jauge du Steam Controller est recréée à chaque reconnexion : on relit
/// sa power_supply sans refaire l'énumération complète des nœuds evdev.
fn refresh_steam_battery(device: &mut DeviceInfo) {
    if device.vendor != 0x28de || !matches!(device.product, 0x1304 | 0x1305) {
        return;
    }
    let Some(dir) = battery_supply(&device.path) else {
        return;
    };
    device.battery = read_capacity(&dir);
    device.battery_status = read_status(&dir);
}

pub fn list_gamepads() -> Vec<DeviceInfo> {
    list().into_iter().filter(|d| d.gamepad).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steam_puck_ids_are_recognised() {
        assert!(is_steam_puck(0x28de, 0x1304));
        assert!(is_steam_puck(0x28de, 0x1305));
        assert!(!is_steam_puck(0x28de, 0x1142));
        assert!(!is_steam_puck(0x054c, 0x0ce6));
    }

    #[test]
    fn only_the_bridge_virtual_pad_is_flagged() {
        assert!(is_steam_bridge_pad(
            0x045e,
            0x028e,
            DeviceLink::Virtual,
            "Microsoft X-Box 360 pad",
            true
        ));
        // Un vrai pad Xbox 360 est en USB, jamais virtuel.
        assert!(!is_steam_bridge_pad(
            0x045e,
            0x028e,
            DeviceLink::Usb,
            "Microsoft X-Box 360 pad",
            true
        ));
        // Sans le pont, un pad virtuel de même nom n'est pas la Steam.
        assert!(!is_steam_bridge_pad(
            0x045e,
            0x028e,
            DeviceLink::Virtual,
            "Microsoft X-Box 360 pad",
            false
        ));
        assert!(!is_steam_bridge_pad(
            0x28de,
            0x1304,
            DeviceLink::Virtual,
            "Steam Controller",
            true
        ));
    }

    #[test]
    fn bridge_process_name_is_recognised_after_truncation() {
        assert!(is_bridge_process_name("steam-puck-brid\n"));
        assert!(is_bridge_process_name("steam-puck-bridge\n"));
        assert!(!is_bridge_process_name("lapinux\n"));
        assert!(!is_bridge_process_name("\n"));
    }

    #[test]
    fn third_party_xbox_ids_are_treated_as_generic() {
        assert!(is_generic_xbox(0x045e, 0x028e, Some("Homertech")));
        assert!(is_generic_xbox(0x045e, 0x028e, Some("MICREAL")));
        assert!(!is_generic_xbox(
            0x045e,
            0x028e,
            Some("Microsoft")
        ));
        // Un pad virtuel sans fabricant USB n'est pas un clone identifié.
        assert!(!is_generic_xbox(0x045e, 0x028e, None));
        assert!(!is_generic_xbox(0x045e, 0x028e, Some("  ")));
        assert!(!is_generic_xbox(0x045e, 0x02d1, Some("Homertech")));
    }

    #[test]
    fn blank_pad_names_fall_back_to_the_link() {
        // evdev peut annoncer un nom vide ou fait d'espaces : la carte doit
        // rester lisible et ne jamais afficher « périphérique inconnu ».
        assert_eq!(
            display_name(Some("   "), true, DeviceLink::Usb),
            "Manette USB"
        );
        assert_eq!(
            display_name(None, true, DeviceLink::Bluetooth),
            "Manette Bluetooth"
        );
        assert_eq!(
            display_name(Some("\t"), true, DeviceLink::Dongle),
            "Manette 2,4 GHz"
        );
        assert_eq!(
            display_name(None, true, DeviceLink::Virtual),
            "Manette virtuelle"
        );
        assert_eq!(
            display_name(Some("DualSense"), true, DeviceLink::Bluetooth),
            "DualSense"
        );
        // Un clavier sans nom n'est pas une manette : il garde le libellé
        // générique, et les noms réels ne sont jamais réécrits.
        assert_eq!(
            display_name(None, false, DeviceLink::Usb),
            "périphérique inconnu"
        );
    }

    #[test]
    fn fallback_names_follow_the_link() {
        assert_eq!(fallback_pad_name(DeviceLink::Usb), "Manette USB");
        assert_eq!(fallback_pad_name(DeviceLink::Bluetooth), "Manette Bluetooth");
        assert_eq!(fallback_pad_name(DeviceLink::Dongle), "Manette 2,4 GHz");
        assert_eq!(fallback_pad_name(DeviceLink::Virtual), "Manette virtuelle");
        assert_eq!(fallback_pad_name(DeviceLink::Unknown), "Manette");
    }

    #[test]
    fn hid_bus_is_recognised() {
        assert_eq!(parse_hid_bus("0003:0000045E:0000028E"), DeviceLink::Usb);
        assert_eq!(parse_hid_bus("0005:0000054C:00000CE6"), DeviceLink::Bluetooth);
        assert_eq!(parse_hid_bus("0018:0000:0000"), DeviceLink::Unknown);
        assert_eq!(parse_hid_bus(""), DeviceLink::Unknown);
    }

    #[test]
    fn usb_cable_and_receiver_are_distinguished() {
        assert!(!is_usb_receiver(
            0x054c,
            0x0ce6,
            "DualSense Wireless Controller",
            "Sony Interactive Entertainment",
        ));
        assert!(is_usb_receiver(
            0x28de,
            0x1304,
            "Steam Controller Puck",
            "Valve Software",
        ));
        assert!(is_usb_receiver(
            0x045e,
            0x02e6,
            "Xbox Wireless Adapter",
            "Microsoft",
        ));
        assert!(is_usb_receiver(
            0x046d,
            0xc21f,
            "Wireless Gamepad F710",
            "Logitech",
        ));
        assert!(is_usb_receiver(0x1234, 0x5678, "USB Receiver", ""));
        assert!(is_usb_receiver(
            0x2dc8,
            0x3106,
            "8BitDo Ultimate 2.4G",
            "8BitDo"
        ));
        assert!(is_usb_receiver(
            0x045e,
            0x0719,
            "Xbox 360 Wireless Receiver",
            "Microsoft"
        ));
        assert!(!is_usb_receiver(0x057e, 0x2009, "Pro Controller", "Nintendo"));
    }

    #[test]
    fn bluetooth_addresses_are_recognised() {
        assert!(is_mac("44:46:48:2e:38:09"));
        assert!(is_mac("AA:BB:CC:DD:EE:FF"));
        assert!(!is_mac("FXB9954802F92"));
        assert!(!is_mac("44:46:48:2e:38"));
        assert!(!is_mac(""));
    }

    #[test]
    fn udev_encoded_strings_are_decoded() {
        assert_eq!(
            decode_udev("MICREAL\\x20GAMEPAD\\x20for\\x20XBox"),
            "MICREAL GAMEPAD for XBox"
        );
        assert_eq!(decode_udev("Homertech"), "Homertech");
        assert_eq!(decode_udev("Contr\\xC3\\xB4leur"), "Contrôleur");
    }

    #[test]
    fn battery_capacity_is_clamped() {
        assert_eq!(parse_capacity("87\n"), Some(87));
        assert_eq!(parse_capacity("150"), Some(100));
        assert_eq!(parse_capacity("n/a"), None);
    }

    #[test]
    fn steam_battery_placeholder_is_not_a_full_charge() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("capacity"), "100\n").unwrap();
        std::fs::write(dir.path().join("voltage_now"), "3000000\n").unwrap();
        assert_eq!(read_capacity(dir.path()), None);

        // Une vraie batterie pleine a une tension de plusieurs volts.
        std::fs::write(dir.path().join("voltage_now"), "4200000\n").unwrap();
        assert_eq!(read_capacity(dir.path()), Some(100));
    }

    #[test]
    fn battery_level_falls_back_to_capacity_level() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("capacity_level"), "High\n").unwrap();
        assert_eq!(read_capacity(dir.path()), Some(75));
        std::fs::write(dir.path().join("capacity_level"), "Full\n").unwrap();
        assert_eq!(read_capacity(dir.path()), Some(100));
        std::fs::write(dir.path().join("capacity_level"), "Unknown\n").unwrap();
        assert_eq!(read_capacity(dir.path()), None);
        // `capacity` reste prioritaire quand le firmware le publie.
        std::fs::write(dir.path().join("capacity"), "62\n").unwrap();
        assert_eq!(read_capacity(dir.path()), Some(62));
        // Branchée, la jauge ne doit pas annoncer 100 % avant la fin.
        std::fs::remove_file(dir.path().join("capacity")).unwrap();
        std::fs::write(dir.path().join("capacity_level"), "Full\n").unwrap();
        std::fs::write(dir.path().join("status"), "Charging\n").unwrap();
        assert_eq!(read_capacity(dir.path()), Some(99));
    }

    #[test]
    fn battery_status_is_read_from_power_supply() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("status"), "Charging\n").unwrap();
        assert_eq!(read_status(dir.path()).as_deref(), Some("Charging"));
        std::fs::write(dir.path().join("status"), "Unknown\n").unwrap();
        assert_eq!(read_status(dir.path()).as_deref(), Some("Unknown"));
        std::fs::write(dir.path().join("status"), "\n").unwrap();
        assert_eq!(read_status(dir.path()), None);
    }

    #[test]
    fn a_repeated_listing_reuses_the_same_devices() {
        let paths = |devices: Vec<DeviceInfo>| -> Vec<PathBuf> {
            devices.into_iter().map(|device| device.path).collect()
        };
        let first = paths(list_cached());
        let second = paths(list_cached());
        assert_eq!(first, second);
    }

    #[test]
    fn enumerates_without_panicking() {
        // Aucune manette n'est garantie sur la machine de test : on vérifie
        // seulement que l'énumération fonctionne et que les champs sont cohérents.
        let devices = list();
        for device in &devices {
            assert!(!device.name.is_empty());
            assert!(device.path.starts_with("/dev/input"));
        }
    }
}
