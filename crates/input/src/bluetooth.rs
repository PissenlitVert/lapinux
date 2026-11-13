//! Manettes Bluetooth connues de BlueZ, y compris celles qui sont appairées
//! mais actuellement hors ligne.
//!
//! evdev ne montre que les manettes connectées. Pour distinguer « appairée »
//! de « connectée » et lister une manette éteinte, on interroge `bluetoothctl`.
//! La commande est non interactive et bornée dans le temps.

use lapinux_core::model::{DeviceInfo, DeviceLink};
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const COMMAND_TIMEOUT: Duration = Duration::from_millis(900);

/// Informations BlueZ d'une manette Bluetooth : de quoi décrire la liaison
/// sans passer par un outil privilégié. Le RSSI et la puissance d'émission ne
/// sont renseignés que si BlueZ les publie (liaisons LE) : une manette HID
/// classique ne les expose pas sans droits administrateur.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BluetoothDetails {
    pub address: String,
    pub address_type: String,
    pub name: String,
    pub class: String,
    pub icon: String,
    pub paired: bool,
    pub bonded: bool,
    pub trusted: bool,
    pub blocked: bool,
    pub connected: bool,
    pub wake_allowed: bool,
    pub legacy_pairing: bool,
    pub cable_pairing: bool,
    pub modalias: String,
    pub hid: bool,
    /// HID over GATT : la manette parle Bluetooth LE plutôt que BR/EDR.
    pub le_hid: bool,
    pub rssi: Option<i16>,
    pub tx_power: Option<i16>,
    pub battery: Option<u8>,
}

impl BluetoothDetails {
    /// Transport réellement utilisé par la liaison HID.
    pub fn link_label(&self) -> &'static str {
        match (self.hid, self.le_hid) {
            (true, false) => "HID classique (BR/EDR)",
            (false, true) => "HID over GATT (LE)",
            (true, true) => "HID classique et LE",
            (false, false) => "Non identifié",
        }
    }

    /// Classe Bluetooth décodée : « Manette », « Joystick »…
    pub fn class_label(&self) -> Option<&'static str> {
        class_label(&self.class)
    }

    /// Libellé de l'état d'appairage, de confiance comprise.
    pub fn pairing_label(&self) -> &'static str {
        match (self.paired, self.trusted) {
            (true, true) => "Oui · de confiance",
            (true, false) => "Oui",
            (false, _) => "Non",
        }
    }
}

/// Lit les propriétés BlueZ d'une adresse. `bluetoothctl info` est borné dans
/// le temps ; l'appel doit rester hors du fil d'affichage.
pub fn details(mac: &str) -> Option<BluetoothDetails> {
    let text = bluetoothctl(&["info", mac])?;
    Some(parse_details(mac, &text))
}

/// Éteint une manette Bluetooth en fermant sa liaison HID.
///
/// Aucune commande constructeur documentée n'éteint une DualSense depuis
/// l'hôte : comme Steam, on coupe le lien. La manette perd son hôte, éteint
/// ses LED puis s'arrête au bout de quelques secondes. BlueZ conserve
/// l'appairage, donc une pression sur PS la rallume et la rebranche seule.
/// L'appel prend quelques dizaines de millisecondes et doit rester hors du
/// fil d'affichage.
pub fn power_off(mac: &str) -> Result<(), String> {
    if mac.trim().is_empty() {
        return Err("adresse Bluetooth inconnue".to_string());
    }
    let (status, text, errors) = bluetoothctl_action(&["disconnect", mac], Duration::from_secs(4))?;
    power_off_result(status.success(), &text, &errors)
}

/// Adresses Bluetooth à couper parce que la même manette est aussi branchée.
///
/// Une DualSense garde sa liaison sans fil quand on la branche pour la
/// charger : Linux expose alors la manette deux fois et ses entrées arrivent
/// par les deux liens. Dès qu'une liaison filaire du même modèle est visible,
/// le câble doit prendre le relais (comme dans Steam) : une seule entrée,
/// moins de latence, et le micro comme le haut-parleur USB deviennent
/// utilisables. Deux manettes identiques dont une seule est branchée restent
/// le seul cas ambigu ; le câble gagne, comme dans Steam.
pub fn wired_takeover(devices: &[DeviceInfo]) -> Vec<String> {
    let mut out = Vec::new();
    for wireless in devices {
        if wireless.connection != DeviceLink::Bluetooth || !wireless.connected || !wireless.gamepad
        {
            continue;
        }
        let Some(mac) = wireless
            .address
            .as_deref()
            .filter(|mac| !mac.trim().is_empty())
        else {
            continue;
        };
        let wired = devices.iter().any(|other| {
            other.connection == DeviceLink::Usb
                && other.gamepad
                && other.vendor == wireless.vendor
                && other.product == wireless.product
                // L'adresse USB (HID_UNIQ) évite de confondre deux manettes
                // identiques dont une seule est branchée.
                && other
                    .address
                    .as_deref()
                    .map_or(true, |address| address.eq_ignore_ascii_case(mac))
        });
        if wired {
            out.push(mac.to_string());
        }
    }
    out
}

/// Veille en tâche de fond : dès qu'une manette est branchée, sa liaison
/// Bluetooth est coupée au profit du câble, même si l'utilisateur est sur une
/// autre page. Quand le noyau a refusé le câble parce que le Bluetooth était
/// encore actif, l'interface USB est réénumérée après la coupure. Une adresse
/// qui vient d'échouer n'est retentée qu'une minute plus tard pour ne pas
/// marteler BlueZ.
pub fn watch_wired_takeover() {
    const INTERVAL: Duration = Duration::from_secs(1);
    const RETRY: Duration = Duration::from_secs(60);
    // Un périphérique peut apparaître sans que `/dev/input` change de date
    // (changement de propriétaire d'un nœud existant) : un contrôle complet
    // garde ce cas, assez rare pour ne pas coûter cher.
    const FULL: Duration = Duration::from_secs(30);
    let _ = std::thread::Builder::new()
        .name("lapinux-filaire".into())
        .spawn(|| {
            let mut attempted: HashMap<String, Instant> = HashMap::new();
            let mut signature = input_signature();
            // Le premier tour inspecte tout de suite : une manette déjà
            // branchée au lancement ne doit pas attendre le contrôle complet.
            let mut last_full = Instant::now() - FULL;
            loop {
                // Énumérer les périphériques ouvre chaque nœud evdev : le faire
                // chaque seconde coûtait près d'un pour cent d'un cœur, en
                // permanence. La date du dossier suffit à savoir qu'une
                // manette vient d'apparaître.
                let current = input_signature();
                if current != signature || last_full.elapsed() >= FULL {
                    signature = current;
                    last_full = Instant::now();
                    for mac in wired_takeover(&crate::device::list()) {
                        if attempted.get(&mac).map_or(false, |at| at.elapsed() < RETRY) {
                            continue;
                        }
                        attempted.insert(mac.clone(), Instant::now());
                        match power_off(&mac) {
                            Ok(()) => {
                                attempted.remove(&mac);
                                tracing::info!(
                                    "{mac} branchée : Bluetooth coupé au profit du câble"
                                );
                            }
                            Err(err) => {
                                tracing::debug!("bascule filaire de {mac} impossible : {err}")
                            }
                        }
                    }
                    attempted.retain(|_, at| at.elapsed() < Duration::from_secs(600));
                }
                std::thread::sleep(INTERVAL);
            }
        });
}

/// Signature bon marché de `/dev/input` : elle change dès qu'une manette est
/// branchée, débranchée ou appairée. Deux appels système, sans ouvrir un seul
/// périphérique.
fn input_signature() -> Option<(std::time::SystemTime, usize)> {
    let modified = std::fs::metadata("/dev/input").ok()?.modified().ok()?;
    let count = std::fs::read_dir("/dev/input").ok()?.count();
    Some((modified, count))
}

/// Tente de reconnecter une manette appairée.
///
/// BlueZ réveille une manette qui écoute encore (liaison perdue, portée) ;
/// une manette vraiment éteinte a coupé sa radio et ne peut être rallumée que
/// par sa touche PS ou son câble. L'échec est donc rendu à l'interface avec
/// une explication plutôt que masqué.
pub fn connect(mac: &str) -> Result<(), String> {
    if mac.trim().is_empty() {
        return Err("adresse Bluetooth inconnue".to_string());
    }
    // BlueZ laisse jusqu'à 30 s à une page Bluetooth pour aboutir. Couper
    // plus tôt rendrait « BlueZ ne répond pas » alors que la manette peut
    // encore être rallumée pendant l'attente.
    let (status, text, errors) = bluetoothctl_action(&["connect", mac], Duration::from_secs(35))
        .map_err(|_| "la manette n'a pas répondu".to_string())?;
    connect_result(status.success(), &text, &errors)
}

/// Lance `bluetoothctl` avec une borne de temps, sans bloquer l'interface :
/// l'appel vit dans un fil de travail, mais BlueZ peut rester muet.
fn bluetoothctl_action(
    args: &[&str],
    timeout: Duration,
) -> Result<(std::process::ExitStatus, String, String), String> {
    let mut child = Command::new("bluetoothctl")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("bluetoothctl : {error}"))?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("BlueZ ne répond pas".to_string());
            }
        }
    };
    let mut text = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut text);
    }
    let mut errors = String::new();
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut errors);
    }
    Ok((status, text, errors))
}

/// Interprète la sortie de `bluetoothctl disconnect`. BlueZ 5.87 répond
/// « Disconnection successful » et sort en 0 ; sur un périphérique inconnu ou
/// un lien déjà rompu, il sort en erreur et donne la raison.
fn power_off_result(success: bool, stdout: &str, stderr: &str) -> Result<(), String> {
    if success && stdout.contains("successful") {
        return Ok(());
    }
    let reason = stderr
        .lines()
        .chain(stdout.lines())
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .find(|line| line.contains("Failed") || line.contains("not available"))
        .map(|line| {
            line.trim_start_matches("Failed to disconnect: ")
                .to_string()
        });
    Err(reason.unwrap_or_else(|| "la liaison n'a pas été coupée".to_string()))
}

/// Interprète la sortie de `bluetoothctl connect`. Les erreurs BlueZ sont
/// traduites pour l'interface ; une manette éteinte répond
/// `br-connection-create-socket`, ce qui signifie que sa radio est coupée.
fn connect_result(success: bool, stdout: &str, stderr: &str) -> Result<(), String> {
    if success && stdout.contains("successful") {
        return Ok(());
    }
    let reason = stderr
        .lines()
        .chain(stdout.lines())
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .find(|line| line.contains("Failed") || line.contains("not available"))
        .map(|line| {
            line.trim_start_matches("Failed to connect: ")
                .trim_start_matches("org.bluez.Error.Failed ")
                .to_string()
        });
    Err(match reason.as_deref() {
        Some(reason) if reason.contains("br-connection-create-socket") => {
            "manette éteinte ou hors de portée : appuie sur PS ou branche le câble".to_string()
        }
        Some(reason) if reason.contains("InProgress") || reason.contains("in progress") => {
            "connexion déjà en cours".to_string()
        }
        Some(reason) if reason.contains("not available") => "manette introuvable".to_string(),
        Some(reason) => reason.to_string(),
        None => "la manette n'a pas répondu".to_string(),
    })
}

/// Une manette Bluetooth appairée, connectée ou non.
pub fn paired_gamepads() -> Vec<DeviceInfo> {
    let Some(paired) = bluetoothctl(&["devices", "Paired"]) else {
        return Vec::new();
    };
    let connected = bluetoothctl(&["devices", "Connected"])
        .map(|text| parse_macs(&text))
        .unwrap_or_default();
    let mut out = Vec::new();
    for (mac, fallback_name) in parse_paired(&paired) {
        let Some(info) = bluetoothctl(&["info", &mac]) else {
            continue;
        };
        let Some(mut pad) = parse_info(&mac, &info, &fallback_name, &connected) else {
            continue;
        };
        if pad.gamepad {
            pad.connection = DeviceLink::Bluetooth;
            out.push(pad);
        }
    }
    out.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.address.cmp(&b.address))
    });
    out
}

fn bluetoothctl(args: &[&str]) -> Option<String> {
    let mut child = Command::new("bluetoothctl")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + COMMAND_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                // BlueZ peut rester bloqué si le service D-Bus ne répond pas.
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut text = String::new();
    child.stdout.take()?.read_to_string(&mut text).ok()?;
    Some(text)
}

/// Lignes `Device AA:BB:CC:DD:EE:FF Nom de la manette`.
fn parse_paired(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("Device ")?;
            let (mac, name) = rest.split_once(' ')?;
            let mac = mac.to_ascii_uppercase();
            (!mac.is_empty()).then(|| (mac, name.trim().to_string()))
        })
        .collect()
}

fn parse_macs(text: &str) -> Vec<String> {
    parse_paired(text).into_iter().map(|(mac, _)| mac).collect()
}

fn parse_details(mac: &str, text: &str) -> BluetoothDetails {
    let address_type = text
        .lines()
        .next()
        .and_then(|line| line.split_once('('))
        .and_then(|(_, rest)| rest.split_once(')'))
        .map(|(value, _)| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "public".to_string());
    let class = field(text, "Class")
        .and_then(|value| value.split_whitespace().next().map(String::from))
        .unwrap_or_default();
    BluetoothDetails {
        address: mac.to_ascii_uppercase(),
        address_type,
        name: field(text, "Alias")
            .or_else(|| field(text, "Name"))
            .unwrap_or_default(),
        class,
        icon: field(text, "Icon").unwrap_or_default(),
        paired: yes(text, "Paired"),
        bonded: yes(text, "Bonded"),
        trusted: yes(text, "Trusted"),
        blocked: yes(text, "Blocked"),
        connected: yes(text, "Connected"),
        wake_allowed: yes(text, "WakeAllowed"),
        legacy_pairing: yes(text, "LegacyPairing"),
        cable_pairing: yes(text, "CablePairing"),
        modalias: field(text, "Modalias").unwrap_or_default(),
        hid: text.contains("00001124-0000-1000-8000-00805f9b34fb"),
        le_hid: text.contains("00001812-0000-1000-8000-00805f9b34fb"),
        rssi: field(text, "RSSI").and_then(|value| parse_signed(&value)),
        tx_power: field(text, "TxPower").and_then(|value| parse_signed(&value)),
        battery: field(text, "Battery Percentage").and_then(|value| parse_battery(&value)),
    }
}

/// `-61` ou `0xffffffc4 (-61)` → −61.
fn parse_signed(value: &str) -> Option<i16> {
    let raw = value.split_whitespace().next()?;
    let number = match raw.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16).ok()? as i32,
        None => raw.parse().ok()?,
    };
    i16::try_from(number).ok()
}

/// Sous-classe d'un périphérique Bluetooth (classe majeure 0x05).
fn class_label(text: &str) -> Option<&'static str> {
    let raw = text.split_whitespace().next()?;
    let class = u32::from_str_radix(raw.trim_start_matches("0x"), 16).ok()?;
    let major = (class >> 8) & 0x1f;
    let minor = (class >> 2) & 0x3f;
    if major != 0x05 {
        return None;
    }
    Some(match minor {
        0x01 => "Joystick",
        0x02 => "Manette",
        0x03 => "Télécommande",
        _ => "Périphérique",
    })
}

fn yes(text: &str, key: &str) -> bool {
    field(text, key)
        .map(|value| value == "yes")
        .unwrap_or(false)
}

/// `0x5f (95)` → 95.
fn parse_battery(value: &str) -> Option<u8> {
    if let Some((_, rest)) = value.split_once('(') {
        if let Some((number, _)) = rest.split_once(')') {
            return number.trim().parse().ok();
        }
    }
    let raw = value.trim();
    if let Some(hex) = raw.strip_prefix("0x") {
        return u8::from_str_radix(hex, 16).ok();
    }
    raw.parse().ok()
}

fn parse_info(
    mac: &str,
    text: &str,
    fallback_name: &str,
    connected: &[String],
) -> Option<DeviceInfo> {
    let mut name = field(text, "Alias")
        .or_else(|| field(text, "Name"))
        .unwrap_or_else(|| fallback_name.trim().to_string());
    if name.trim().is_empty() {
        name = crate::device::fallback_pad_name(DeviceLink::Bluetooth).to_string();
    }
    let icon = field(text, "Icon").unwrap_or_default();
    let class = field(text, "Class").unwrap_or_default();
    if icon != "input-gaming" && !class_is_gamepad(&class) {
        return None;
    }
    let online = field(text, "Connected")
        .map(|value| value == "yes")
        .unwrap_or_else(|| connected.iter().any(|known| known == mac));
    let (vendor, product) = field(text, "Modalias")
        .map(|value| parse_modalias(&value))
        .unwrap_or((0, 0));
    Some(DeviceInfo {
        path: PathBuf::new(),
        name,
        vendor,
        product,
        gamepad: true,
        buttons: 0,
        button_names: Vec::new(),
        axes: Vec::new(),
        force_feedback: false,
        touchpad: false,
        connection: DeviceLink::Bluetooth,
        // BlueZ expose la batterie de la plupart des manettes récentes,
        // même quand le pilote evdev ne crée pas de power_supply.
        battery: field(text, "Battery Percentage").and_then(|value| parse_battery(&value)),
        battery_status: None,
        address: Some(mac.to_string()),
        connected: online,
        steam_bridge: false,
        udev: Default::default(),
    })
}

fn field(text: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    text.lines().find_map(|line| {
        let value = line.trim().strip_prefix(&prefix)?.trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// Classe Bluetooth : périphérique (0x05), mineure joystick (0x01) ou
/// manette (0x02).
fn class_is_gamepad(text: &str) -> bool {
    let Some(raw) = text.split_whitespace().next() else {
        return false;
    };
    let Ok(class) = u32::from_str_radix(raw.trim_start_matches("0x"), 16) else {
        return false;
    };
    let major = (class >> 8) & 0x1f;
    let minor = (class >> 2) & 0x3f;
    major == 0x05 && matches!(minor, 0x01 | 0x02)
}

/// `usb:v054Cp0CE6d0100` → (0x054c, 0x0ce6).
fn parse_modalias(text: &str) -> (u16, u16) {
    let Some(rest) = text.strip_prefix("usb:v") else {
        return (0, 0);
    };
    let Some((vendor, rest)) = rest.split_once('p') else {
        return (0, 0);
    };
    let product = rest
        .split_once('d')
        .map(|(product, _)| product)
        .unwrap_or(rest);
    match (
        u16::from_str_radix(vendor, 16),
        u16::from_str_radix(product, 16),
    ) {
        (Ok(vendor), Ok(product)) => (vendor, product),
        _ => (0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paired_lines_keep_names_with_spaces() {
        let text = "Device 02:00:00:00:00:01 DualSense Wireless Controller\n\
                    Device 02:00:00:00:00:02 Pro Controller\n\
                    Device 02:00:00:00:00:03 Téléphone Exemple\n";
        let pads = parse_paired(text);
        assert_eq!(pads.len(), 3);
        assert_eq!(pads[0].0, "02:00:00:00:00:01");
        assert_eq!(pads[0].1, "DualSense Wireless Controller");
        assert_eq!(pads[2].1, "Téléphone Exemple");
    }

    #[test]
    fn only_gaming_devices_are_kept() {
        let duel = "Device 02:00:00:00:00:01 (public)\n\
                    \tName: DualSense Wireless Controller\n\
                    \tAlias: DualSense Wireless Controller\n\
                    \tClass: 0x00002508 (9480)\n\
                    \tIcon: input-gaming\n\
                    \tConnected: no\n\
                    \tModalias: usb:v054Cp0CE6d0100\n";
        let pad = parse_info("02:00:00:00:00:01", duel, "Manette", &[]).unwrap();
        assert_eq!(pad.name, "DualSense Wireless Controller");
        assert_eq!(pad.vendor, 0x054c);
        assert_eq!(pad.product, 0x0ce6);
        assert!(!pad.connected);
        assert_eq!(pad.connection, DeviceLink::Bluetooth);

        let headset = "Name: UGREEN HiTune\nClass: 0x00240404\nIcon: audio-headset\n";
        assert!(parse_info("02:00:00:00:00:04", headset, "HiTune", &[]).is_none());
    }

    #[test]
    fn class_and_modalias_are_decoded() {
        assert!(class_is_gamepad("0x00002508 (9480)"));
        assert!(class_is_gamepad("0x00000508 (1288)"));
        assert!(!class_is_gamepad("0x00240404 (2360324)"));
        assert!(!class_is_gamepad(""));
        assert_eq!(parse_modalias("usb:v057Ep2009d0001"), (0x057e, 0x2009));
        assert_eq!(parse_modalias("nonsense"), (0, 0));
    }

    #[test]
    fn bluetooth_details_read_the_bluez_fields() {
        let text = "Device 02:00:00:00:00:01 (public)\n\
                    \tName: DualSense Wireless Controller\n\
                    \tAlias: DualSense Wireless Controller\n\
                    \tClass: 0x00002508 (9480)\n\
                    \tIcon: input-gaming\n\
                    \tPaired: yes\n\
                    \tBonded: yes\n\
                    \tTrusted: yes\n\
                    \tBlocked: no\n\
                    \tConnected: yes\n\
                    \tWakeAllowed: yes\n\
                    \tLegacyPairing: no\n\
                    \tCablePairing: no\n\
                    \tUUID: Human Interface Device... (00001124-0000-1000-8000-00805f9b34fb)\n\
                    \tModalias: usb:v054Cp0CE6d0100\n\
                    \tBattery Percentage: 0x5f (95)\n";
        let details = parse_details("02:00:00:00:00:01", text);
        assert_eq!(details.address, "02:00:00:00:00:01");
        assert_eq!(details.address_type, "public");
        assert_eq!(details.name, "DualSense Wireless Controller");
        assert_eq!(details.class, "0x00002508");
        assert!(details.paired && details.bonded && details.trusted);
        assert!(!details.blocked);
        assert!(details.connected && details.wake_allowed);
        assert!(!details.legacy_pairing && !details.cable_pairing);
        assert!(details.hid && !details.le_hid);
        assert_eq!(details.battery, Some(95));
        assert_eq!(details.link_label(), "HID classique (BR/EDR)");
        assert_eq!(details.class_label(), Some("Manette"));
        assert_eq!(details.pairing_label(), "Oui · de confiance");
        assert_eq!(details.rssi, None);
    }

    #[test]
    fn paired_pad_without_name_gets_a_link_fallback() {
        let text = "Device AA:BB:CC:DD:EE:FF (public)\n\
                    \tIcon: input-gaming\n\
                    \tConnected: yes\n";
        let info = parse_info("AA:BB:CC:DD:EE:FF", text, "", &[]).unwrap();
        assert_eq!(info.name, "Manette Bluetooth");
    }

    #[test]
    fn le_link_and_signal_are_decoded() {
        let text = "Device 02:00:00:00:00:02 (random)\n\
                    \tClass: 0x00002508 (9480)\n\
                    \tUUID: Human Interface Device... (00001812-0000-1000-8000-00805f9b34fb)\n\
                    \tRSSI: -61\n\
                    \tTxPower: 0x04 (4)\n";
        let details = parse_details("02:00:00:00:00:02", text);
        assert!(details.le_hid && !details.hid);
        assert_eq!(details.link_label(), "HID over GATT (LE)");
        assert_eq!(details.rssi, Some(-61));
        assert_eq!(details.tx_power, Some(4));
        assert_eq!(details.pairing_label(), "Non");

        assert_eq!(parse_signed("0xffffffc4 (-60)"), Some(-60));
        assert_eq!(parse_signed("-54"), Some(-54));
        assert_eq!(class_label("0x00000508"), Some("Manette"));
        assert_eq!(class_label("0x00000504"), Some("Joystick"));
        assert_eq!(class_label("0x00240404"), None);
    }

    #[test]
    fn power_off_reads_bluetoothctl_outcome() {
        assert_eq!(
            power_off_result(
                true,
                "Attempting to disconnect from 02:00:00:00:00:01\nDisconnection successful\n",
                ""
            ),
            Ok(())
        );
        assert_eq!(
            power_off_result(false, "", "Failed to disconnect: Not connected\n"),
            Err("Not connected".to_string())
        );
        assert_eq!(
            power_off_result(false, "Device 00:00:00:00:00:00 not available\n", ""),
            Err("Device 00:00:00:00:00:00 not available".to_string())
        );
        assert_eq!(
            power_off_result(false, "", ""),
            Err("la liaison n'a pas été coupée".to_string())
        );
        assert!(power_off("  ").is_err());
    }

    fn pad(name: &str, link: DeviceLink, address: Option<&str>) -> DeviceInfo {
        DeviceInfo {
            path: PathBuf::new(),
            name: name.to_string(),
            vendor: 0x054c,
            product: 0x0ce6,
            gamepad: true,
            buttons: 18,
            button_names: Vec::new(),
            axes: Vec::new(),
            force_feedback: false,
            touchpad: false,
            connection: link,
            battery: None,
            battery_status: None,
            address: address.map(str::to_string),
            connected: true,
            steam_bridge: false,
            udev: Default::default(),
        }
    }

    #[test]
    fn a_wired_pad_takes_over_from_its_bluetooth_link() {
        let wireless = pad(
            "DualSense",
            DeviceLink::Bluetooth,
            Some("02:00:00:00:00:01"),
        );
        let wired = pad("DualSense", DeviceLink::Usb, None);
        assert_eq!(
            wired_takeover(&[wireless.clone(), wired.clone()]),
            vec!["02:00:00:00:00:01".to_string()]
        );

        // Seule, la liaison sans fil reste en place.
        assert!(wired_takeover(&[wireless.clone()]).is_empty());
        // Un autre modèle branché ne coupe pas la manette sans fil.
        let mut other = pad("DualSense", DeviceLink::Usb, None);
        other.product = 0x0df2;
        assert!(wired_takeover(&[wireless.clone(), other]).is_empty());
        // Une autre DualSense du même modèle, avec une autre adresse, non plus.
        let other = pad("DualSense", DeviceLink::Usb, Some("AA:BB:CC:DD:EE:FF"));
        assert!(wired_takeover(&[wireless.clone(), other]).is_empty());
        // Une entrée appairée mais hors ligne n'a pas de liaison à couper.
        let mut offline = wireless.clone();
        offline.connected = false;
        assert!(wired_takeover(&[offline, wired]).is_empty());
    }

    #[test]
    fn connect_reports_bluez_failures_in_french() {
        assert_eq!(connect_result(true, "Connection successful\n", ""), Ok(()));
        assert_eq!(
            connect_result(
                false,
                "Failed to connect: org.bluez.Error.Failed br-connection-create-socket\n",
                ""
            ),
            Err(
                "manette éteinte ou hors de portée : appuie sur PS ou branche le câble".to_string()
            )
        );
        assert_eq!(
            connect_result(false, "Failed to connect: org.bluez.Error.InProgress\n", ""),
            Err("connexion déjà en cours".to_string())
        );
        assert_eq!(
            connect_result(false, "", ""),
            Err("la manette n'a pas répondu".to_string())
        );
        assert!(connect("  ").is_err());
    }

    #[test]
    fn connected_list_falls_back_when_info_has_no_state() {
        let info = "Name: Pro Controller\nClass: 0x00000508\n";
        let pad = parse_info(
            "02:00:00:00:00:02",
            info,
            "Pro Controller",
            &["02:00:00:00:00:02".to_string()],
        )
        .unwrap();
        assert!(pad.connected);
    }
}
