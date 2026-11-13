//! Batterie des manettes Sony.
//!
//! Le firmware ne transmet pas de pourcentage à 1 % : le rapport d'entrée ne
//! contient qu'un palier de 10 %. En revanche, la commande de test
//! constructeur expose la tension réelle en millivolts et les compteurs de
//! charge. Elle passe par les feature reports hidraw (`0x80` pour la demande,
//! `0x81` pour la réponse), donc elle fonctionne aussi en Bluetooth.
//!
//! Les identifiants de commande viennent du protocole de test DualSense :
//! `ANALOG_DATA/BATTERY` (0x04/0x03) pour la tension, `SYSTEM/READ_BATTERY_BARCODE`
//! (0x01/0x18) pour le code batterie et `TELEMETRY/GET_INFO` (0x70/0x01) pour
//! les compteurs d'usure.

use std::fs::File;
use std::io;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const REPORT_SIZE: usize = 64;
const PAGE_SIZE: usize = 56;
const REQUEST_REPORT: u8 = 0x80;
const RESULT_REPORT: u8 = 0x81;
const STATUS_COMPLETE: u8 = 2;
const STATUS_MORE: u8 = 3;
const STATUS_TIMEOUT: u8 = 0xFF;
const COMMAND_TIMEOUT: Duration = Duration::from_millis(800);
const VOLTAGE_INTERVAL: Duration = Duration::from_secs(5);
const TELEMETRY_INTERVAL: Duration = Duration::from_secs(60);
const FAILED_RETRY: Duration = Duration::from_secs(60);
const WORKER_TICK: Duration = Duration::from_millis(200);

const HIDIOCSFEATURE_64: libc::c_ulong = hid_ioc(0x06, REPORT_SIZE);
const HIDIOCGFEATURE_64: libc::c_ulong = hid_ioc(0x07, REPORT_SIZE);

const fn hid_ioc(number: u8, size: usize) -> libc::c_ulong {
    // _IOC(_IOC_WRITE | _IOC_READ, 'H', number, size)
    ((3_u64 << 30) | ((size as u64) << 16) | ((b'H' as u64) << 8) | number as u64) as libc::c_ulong
}

/// Modèle de batterie connu, utilisé pour afficher la capacité nominale.
/// Le contrôleur ne publie pas cette valeur : elle vient de la fiche Sony.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatteryDesign {
    pub milliamp_hours: u16,
    pub milliwatt_hours: u32,
}

impl BatteryDesign {
    pub fn for_product(product: u16) -> Option<Self> {
        match product {
            // DualSense : LIP1708, 3,65 V.
            0x0ce6 => Some(Self {
                milliamp_hours: 1_560,
                milliwatt_hours: 5_700,
            }),
            // DualSense Edge.
            0x0df2 => Some(Self {
                milliamp_hours: 1_050,
                milliwatt_hours: 3_900,
            }),
            // DualShock 4 v1 et v2, si un jour la fiche les présente.
            0x05c4 | 0x09cc => Some(Self {
                milliamp_hours: 1_000,
                milliwatt_hours: 3_700,
            }),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChargeState {
    #[default]
    Unknown,
    Discharging,
    Charging,
    Full,
    NotCharging,
    VoltageError,
    TemperatureError,
    ChargingError,
}

impl ChargeState {
    /// Le quartet haut du octet d'état du rapport DualSense.
    pub fn from_report(value: u8) -> Self {
        match value {
            0x0 => Self::Discharging,
            0x1 => Self::Charging,
            0x2 => Self::Full,
            0xa => Self::VoltageError,
            0xb => Self::TemperatureError,
            0xf => Self::ChargingError,
            _ => Self::Unknown,
        }
    }

    /// Le texte de `/sys/class/power_supply/.../status`.
    pub fn from_sysfs(value: &str) -> Self {
        match value.trim() {
            "Charging" => Self::Charging,
            "Discharging" => Self::Discharging,
            "Full" => Self::Full,
            "Not charging" => Self::NotCharging,
            _ => Self::Unknown,
        }
    }

    pub fn is_error(self) -> bool {
        matches!(
            self,
            Self::VoltageError | Self::TemperatureError | Self::ChargingError
        )
    }
}

/// Le seul niveau de batterie du rapport d'entrée : 0 à 10 (paliers de 10 %).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatteryReading {
    pub level: u8,
    pub state: ChargeState,
}

impl BatteryReading {
    /// Milieu du palier, comme le pilote Linux. Les états d'erreur ne
    /// publient pas de pourcentage valide.
    ///
    /// « Full » ne veut pas dire 100 % : sur une DualSense branchée, le
    /// firmware annonce « pleine » dès que le chargeur est connecté, alors que
    /// le niveau réel est encore à 80–89 %. Le seul chiffre fiable reste le
    /// quartet de niveau ; on ne le remplace donc plus par 100.
    pub fn percent(self) -> Option<u8> {
        if self.state.is_error() {
            return None;
        }
        (self.level <= 10).then(|| ((self.level as u16 * 10 + 5).min(100)) as u8)
    }
}

/// État complet partagé entre le fil de lecture de la manette et l'interface.
/// Les champs `Option` restent vides quand le firmware ne répond pas.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BatteryInfo {
    pub percent: Option<u8>,
    pub level: Option<u8>,
    pub state: ChargeState,
    /// Tension batterie en millivolts, lue par la commande de test Sony.
    pub voltage_mv: Option<u16>,
    pub cycles: Option<u16>,
    pub full_charges: Option<u16>,
    pub charge_time_s: Option<u32>,
    pub active_time_s: Option<u32>,
    pub charger_connects: Option<u16>,
    pub voltage_errors: Option<u8>,
    pub temperature_errors: Option<u8>,
    pub pmic_errors: Option<u8>,
    /// Code-barres constructeur de la batterie (référence de la pièce).
    pub battery_code: Option<String>,
    pub design: Option<BatteryDesign>,
    pub voltage_at: Option<Instant>,
    pub telemetry_at: Option<Instant>,
}

impl BatteryInfo {
    pub fn apply_report(&mut self, reading: BatteryReading) {
        self.level = Some(reading.level);
        self.state = reading.state;
        self.percent = reading.percent();
    }

    /// Intervalle de 10 % auquel appartient le niveau, ex. `70–79 %`.
    pub fn band(&self) -> Option<(u8, u8)> {
        // Le quartet de niveau est la seule mesure du firmware. « Full » décrit
        // l'alimentation (sur secteur), pas le niveau : l'afficher comme 100 %
        // annonçait « pleine » à 85 %, ce que faisait le pilote.
        let level = self.level.or_else(|| {
            self.percent
                .map(|percent| if percent >= 100 { 10 } else { percent / 10 })
        })?;
        if level > 10 {
            return None;
        }
        Some((level * 10, (level * 10 + 9).min(100)))
    }
}

/// Décode le seul octet batterie du rapport DualSense (USB 0x01 ou BT 0x31).
pub(crate) fn decode_dualsense_battery(data: &[u8]) -> Option<BatteryReading> {
    let base = dualsense_base(data)?;
    let status = *data.get(base + 52)?;
    Some(BatteryReading {
        level: status & 0x0f,
        state: ChargeState::from_report(status >> 4),
    })
}

/// Décode `bat_con` du rapport 0x30 d'une manette Switch : bit 0 = alimentée
/// par le port, bit 4 = en charge, bits 5 à 7 = niveau sur cinq paliers.
/// Ce rapport ne donne que ces paliers, pas un pourcentage à 1 %.
///
/// Le dernier palier (4) est annoncé « pleine » par le firmware dès le
/// branchement, et même parfois hors charge : il ne vaut pas 100 %. Un Pro
/// Controller à ~80 % le rapporte déjà ; on l'affiche donc comme le palier
/// 80–89 %, jamais comme 100 %.
pub(crate) fn decode_nintendo_battery(data: &[u8]) -> Option<BatteryReading> {
    if data.first() != Some(&0x30) {
        return None;
    }
    let value = *data.get(2)?;
    let level = match value >> 5 {
        0 => 0,
        1 => 3,
        2 => 5,
        3 => 8,
        4 => 8,
        _ => return None,
    };
    let state = if value & 0x10 != 0 {
        ChargeState::Charging
    } else if (value >> 5) == 4 && value & 0x01 != 0 {
        ChargeState::Full
    } else {
        ChargeState::Discharging
    };
    Some(BatteryReading { level, state })
}

/// Le rapport Bluetooth 0x31 commence par deux octets de transport, l'USB
/// 0x01 par un seul. Les champs utiles ont ensuite le même décalage.
pub(crate) fn dualsense_base(data: &[u8]) -> Option<usize> {
    match data.first().copied() {
        Some(0x31) if data.len() >= 78 => Some(2),
        Some(0x01) if data.len() >= 64 => Some(1),
        _ => None,
    }
}

struct SonyTestReader {
    file: File,
    bluetooth: bool,
}

impl SonyTestReader {
    fn open(path: &Path, bluetooth: bool) -> io::Result<Self> {
        let file = File::options().read(true).write(true).open(path)?;
        Ok(Self { file, bluetooth })
    }

    fn battery_voltage(&mut self) -> io::Result<u16> {
        let data = self.run_command(0x04, 0x03, 1)?;
        parse_voltage(&data)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "tension invalide"))
    }

    fn battery_code(&mut self) -> io::Result<String> {
        let data = self.run_command(0x01, 0x18, 1)?;
        parse_battery_code(&data)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "code batterie vide"))
    }

    fn telemetry(&mut self) -> io::Result<Telemetry> {
        let data = self.run_command(0x70, 0x01, 4)?;
        parse_telemetry(&data)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "télémétrie incomplète"))
    }

    fn run_command(&mut self, device: u8, action: u8, pages: usize) -> io::Result<Vec<u8>> {
        self.set_test_command(device, action)?;
        let mut out = vec![0u8; pages * PAGE_SIZE];
        let deadline = Instant::now() + COMMAND_TIMEOUT;
        let mut page = 0;
        while page < pages && Instant::now() < deadline {
            let report = self.get_result()?;
            if page > 0 && report[0] == RESULT_REPORT && report[1] == 0 && report[2] == 0 {
                // Le contrôleur efface l'en-tête après la dernière page.
                break;
            }
            if report[0] != RESULT_REPORT || report[1] != device || report[2] != action {
                // Une réponse d'une commande précédente peut encore occuper le
                // slot 0x81 : on la laisse au prochain lecteur.
                std::thread::sleep(Duration::from_millis(6));
                continue;
            }
            match report[3] {
                STATUS_COMPLETE | STATUS_MORE => {
                    out[page * PAGE_SIZE..(page + 1) * PAGE_SIZE]
                        .copy_from_slice(&report[4..4 + PAGE_SIZE]);
                    page += 1;
                    if report[3] == STATUS_COMPLETE {
                        break;
                    }
                }
                STATUS_TIMEOUT => break,
                _ => {}
            }
            std::thread::sleep(Duration::from_millis(6));
        }
        if page == 0 {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "la manette n'a pas répondu",
            ));
        }
        Ok(out)
    }

    fn set_test_command(&mut self, device: u8, action: u8) -> io::Result<()> {
        let mut report = [0u8; REPORT_SIZE];
        report[0] = REQUEST_REPORT;
        report[1] = device;
        report[2] = action;
        if self.bluetooth {
            let crc = feature_crc(&report[..60]);
            report[60..64].copy_from_slice(&crc.to_le_bytes());
        }
        let ret = unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                HIDIOCSFEATURE_64,
                report.as_mut_ptr(),
            )
        };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn get_result(&mut self) -> io::Result<[u8; REPORT_SIZE]> {
        let mut report = [0u8; REPORT_SIZE];
        report[0] = RESULT_REPORT;
        let ret = unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                HIDIOCGFEATURE_64,
                report.as_mut_ptr(),
            )
        };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(report)
    }
}

/// CRC des feature reports Bluetooth Sony (graine 0x53, portée sur l'ID et
/// les 59 premiers octets, les quatre derniers étant le CRC lui-même).
fn feature_crc(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    crc = crc32_byte(crc, 0x53);
    for &byte in data {
        crc = crc32_byte(crc, byte);
    }
    !crc
}

fn crc32_byte(mut crc: u32, byte: u8) -> u32 {
    crc ^= byte as u32;
    for _ in 0..8 {
        crc = if crc & 1 != 0 {
            (crc >> 1) ^ 0xEDB8_8320
        } else {
            crc >> 1
        };
    }
    crc
}

fn parse_voltage(data: &[u8]) -> Option<u16> {
    if data.len() < 2 {
        return None;
    }
    let millivolts = u16::from_le_bytes([data[0], data[1]]);
    (2_500..=4_500).contains(&millivolts).then_some(millivolts)
}

fn parse_battery_code(data: &[u8]) -> Option<String> {
    let code: String = data
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| char::from(*byte))
        .collect();
    let code = code.trim().to_string();
    (!code.is_empty()).then_some(code)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Telemetry {
    active_time_s: u32,
    charge_time_s: u32,
    cycles: u16,
    full_charges: u16,
    charger_connects: u16,
    voltage_errors: u8,
    temperature_errors: u8,
    pmic_errors: u8,
}

fn parse_telemetry(block: &[u8]) -> Option<Telemetry> {
    // Le premier octet du bloc est un statut ; les champs commencent après.
    let data = block.get(1..)?;
    let field = |offset: usize, len: usize| data.get(offset..offset + len);
    Some(Telemetry {
        active_time_s: read_low24(field(21, 4)?),
        charge_time_s: read_low24(field(25, 4)?),
        cycles: read_u16(field(41, 2)?),
        charger_connects: read_u16(field(55, 2)?),
        voltage_errors: *field(57, 1)?.first()?,
        temperature_errors: *field(58, 1)?.first()?,
        pmic_errors: *field(59, 1)?.first()?,
        full_charges: read_u16(field(60, 2)?),
    })
}

fn read_low24(bytes: &[u8]) -> u32 {
    let mut raw = [0u8; 4];
    let len = bytes.len().min(4);
    raw[..len].copy_from_slice(&bytes[..len]);
    let value = u32::from_le_bytes(raw) & 0x00FF_FFFF;
    // Une NVRAM vierge renvoie 0xFFFFFF : ce n'est pas un compteur réel.
    if value < 0x00FF_FF00 {
        value
    } else {
        0
    }
}

fn read_u16(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}

/// Fil de lecture de la batterie, séparé du fil evdev pour qu'une commande
/// de test lente ne retarde jamais les boutons ou le gyroscope.
pub(crate) struct BatteryWorker {
    shared: Arc<Mutex<BatteryInfo>>,
    revision: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl BatteryWorker {
    pub(crate) fn start(path: PathBuf, product: u16, bluetooth: bool) -> Option<Self> {
        let reader = SonyTestReader::open(&path, bluetooth).ok()?;
        let shared = Arc::new(Mutex::new(BatteryInfo {
            design: BatteryDesign::for_product(product),
            ..BatteryInfo::default()
        }));
        let revision = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_shared = Arc::clone(&shared);
        let thread_revision = Arc::clone(&revision);
        let thread_stop = Arc::clone(&stop);
        let join = std::thread::Builder::new()
            .name("lapinux-pad-battery".into())
            .spawn(move || run_worker(reader, thread_shared, thread_revision, thread_stop))
            .ok()?;
        Some(Self {
            shared,
            revision,
            stop,
            join: Some(join),
        })
    }

    pub(crate) fn snapshot(&self) -> BatteryInfo {
        self.shared
            .lock()
            .map(|battery| battery.clone())
            .unwrap_or_default()
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }
}

impl Drop for BatteryWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn run_worker(
    mut reader: SonyTestReader,
    shared: Arc<Mutex<BatteryInfo>>,
    revision: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
) {
    let mut next_voltage = Instant::now();
    let mut next_telemetry = Instant::now();
    let mut next_barcode = Instant::now();
    let mut voltage_failures = 0u32;
    let mut telemetry_failures = 0u32;
    let mut barcode_failures = 0u32;
    let mut barcode_done = false;

    while !stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        let mut changed = false;

        if now >= next_voltage {
            match reader.battery_voltage() {
                Ok(millivolts) => {
                    if let Ok(mut battery) = shared.lock() {
                        battery.voltage_mv = Some(millivolts);
                        battery.voltage_at = Some(Instant::now());
                    }
                    voltage_failures = 0;
                    next_voltage = now + VOLTAGE_INTERVAL;
                    changed = true;
                }
                Err(_) => {
                    voltage_failures += 1;
                    next_voltage = now + backoff(voltage_failures);
                }
            }
        }

        if now >= next_telemetry {
            match reader.telemetry() {
                Ok(telemetry) => {
                    if let Ok(mut battery) = shared.lock() {
                        battery.active_time_s = Some(telemetry.active_time_s);
                        battery.charge_time_s = Some(telemetry.charge_time_s);
                        battery.cycles = Some(telemetry.cycles);
                        battery.full_charges = Some(telemetry.full_charges);
                        battery.charger_connects = Some(telemetry.charger_connects);
                        battery.voltage_errors = Some(telemetry.voltage_errors);
                        battery.temperature_errors = Some(telemetry.temperature_errors);
                        battery.pmic_errors = Some(telemetry.pmic_errors);
                        battery.telemetry_at = Some(Instant::now());
                    }
                    telemetry_failures = 0;
                    next_telemetry = now + TELEMETRY_INTERVAL;
                    changed = true;
                }
                Err(_) => {
                    telemetry_failures += 1;
                    next_telemetry = now + backoff(telemetry_failures);
                }
            }
        }

        if !barcode_done && now >= next_barcode {
            match reader.battery_code() {
                Ok(code) => {
                    if let Ok(mut battery) = shared.lock() {
                        battery.battery_code = Some(code);
                    }
                    barcode_done = true;
                    changed = true;
                }
                Err(_) => {
                    barcode_failures += 1;
                    if barcode_failures >= 3 {
                        // Firmware sans code-barres : inutile de réessayer à
                        // chaque tour.
                        barcode_done = true;
                    } else {
                        next_barcode = now + backoff(barcode_failures);
                    }
                }
            }
        }

        if changed {
            revision.fetch_add(1, Ordering::Relaxed);
        }
        sleep_stoppable(&stop, WORKER_TICK);
    }
}

fn backoff(failures: u32) -> Duration {
    if failures >= 3 {
        FAILED_RETRY
    } else {
        Duration::from_secs(15)
    }
}

fn sleep_stoppable(stop: &AtomicBool, duration: Duration) {
    let mut left = duration;
    while !stop.load(Ordering::Relaxed) && !left.is_zero() {
        let slice = left.min(Duration::from_millis(25));
        std::thread::sleep(slice);
        left -= slice;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dualsense_battery_octet_is_decoded_for_usb_and_bluetooth() {
        for bluetooth in [false, true] {
            let base = if bluetooth { 2 } else { 1 };
            let mut packet = vec![0u8; if bluetooth { 78 } else { 64 }];
            packet[0] = if bluetooth { 0x31 } else { 0x01 };
            packet[base + 52] = 0x17; // en charge, palier 7
            let reading = decode_dualsense_battery(&packet).unwrap();
            assert_eq!(reading.level, 7);
            assert_eq!(reading.state, ChargeState::Charging);
            assert_eq!(reading.percent(), Some(75));
        }
    }

    #[test]
    fn full_and_error_states_do_not_lie_about_the_percentage() {
        let mut packet = vec![0u8; 64];
        packet[0] = 0x01;
        packet[53] = 0x2a; // pleine, palier 10
        let reading = decode_dualsense_battery(&packet).unwrap();
        assert_eq!(reading.state, ChargeState::Full);
        assert_eq!(reading.percent(), Some(100));
        // Cas réel : branchée à 85 %, le firmware annonce déjà « pleine ».
        packet[53] = 0x28;
        let reading = decode_dualsense_battery(&packet).unwrap();
        assert_eq!(reading.state, ChargeState::Full);
        assert_eq!(reading.percent(), Some(85));
        packet[53] = 0xa0; // tension hors plage
        let reading = decode_dualsense_battery(&packet).unwrap();
        assert_eq!(reading.state, ChargeState::VoltageError);
        assert_eq!(reading.percent(), None);
    }

    #[test]
    fn nintendo_battery_byte_has_five_levels() {
        let mut packet = vec![0u8; 49];
        packet[0] = 0x30;
        // Niveau « high » (3), en charge, alimentée.
        packet[2] = 0x71;
        let reading = decode_nintendo_battery(&packet).unwrap();
        assert_eq!(reading.level, 8);
        assert_eq!(reading.state, ChargeState::Charging);
        assert_eq!(reading.percent(), Some(85));
        // Dernier palier, alimentée mais plus en charge : Full ne vaut pas
        // 100 % sur ce firmware, il vaut le palier 80–89 %.
        packet[2] = 0x81;
        let reading = decode_nintendo_battery(&packet).unwrap();
        assert_eq!(reading.level, 8);
        assert_eq!(reading.state, ChargeState::Full);
        assert_eq!(reading.percent(), Some(85));
        // Même niveau sans alimentation : décharge, et surtout pas 100 %
        // (c'est le cas qui faisait sauter la jauge au débranchement).
        packet[2] = 0x80;
        let reading = decode_nintendo_battery(&packet).unwrap();
        assert_eq!(reading.state, ChargeState::Discharging);
        assert_eq!(reading.percent(), Some(85));
        // Dernier palier en charge : le cas réel d'un Pro Controller branché.
        packet[2] = 0x91;
        let reading = decode_nintendo_battery(&packet).unwrap();
        assert_eq!(reading.state, ChargeState::Charging);
        assert_eq!(reading.percent(), Some(85));
        // Trois niveaux ne sont pas des rapports Switch.
        assert!(decode_nintendo_battery(&[0x31; 49]).is_none());
        assert!(decode_nintendo_battery(&[0x30; 2]).is_none());
    }

    #[test]
    fn battery_band_is_the_firmware_step() {
        let mut info = BatteryInfo::default();
        info.apply_report(BatteryReading {
            level: 7,
            state: ChargeState::Charging,
        });
        assert_eq!(info.band(), Some((70, 79)));
        info.apply_report(BatteryReading {
            level: 10,
            state: ChargeState::Full,
        });
        assert_eq!(info.band(), Some((100, 100)));
        // Cas réel d'une DualSense branchée : le quartet reste à 8 alors que
        // le firmware annonce déjà « pleine ». L'interface doit montrer
        // 80–89 %, pas 100 %.
        info.apply_report(BatteryReading {
            level: 8,
            state: ChargeState::Full,
        });
        assert_eq!(info.percent, Some(85));
        assert_eq!(info.band(), Some((80, 89)));
    }

    #[test]
    fn feature_crc_matches_the_controller_vector() {
        let mut report = [0u8; REPORT_SIZE];
        report[0] = REQUEST_REPORT;
        report[1] = 0x04;
        report[2] = 0x03;
        assert_eq!(feature_crc(&report[..60]), 0x3a5c_c978);
    }

    #[test]
    fn telemetry_block_uses_the_battery_offsets() {
        let mut block = vec![0u8; 4 * PAGE_SIZE];
        block[0] = 0;
        let data = &mut block[1..];
        data[21..25].copy_from_slice(&2_377_580u32.to_le_bytes());
        data[25..29].copy_from_slice(&832_789u32.to_le_bytes());
        data[41..43].copy_from_slice(&60u16.to_le_bytes());
        data[55..57].copy_from_slice(&7u16.to_le_bytes());
        data[57] = 1;
        data[58] = 2;
        data[59] = 3;
        data[60..62].copy_from_slice(&4u16.to_le_bytes());
        let telemetry = parse_telemetry(&block).unwrap();
        assert_eq!(telemetry.active_time_s, 2_377_580);
        assert_eq!(telemetry.charge_time_s, 832_789);
        assert_eq!(telemetry.cycles, 60);
        assert_eq!(telemetry.charger_connects, 7);
        assert_eq!(telemetry.voltage_errors, 1);
        assert_eq!(telemetry.temperature_errors, 2);
        assert_eq!(telemetry.pmic_errors, 3);
        assert_eq!(telemetry.full_charges, 4);
    }

    #[test]
    fn voltage_and_barcode_are_sanitised() {
        assert_eq!(parse_voltage(&3_624u16.to_le_bytes()), Some(3_624));
        assert_eq!(parse_voltage(&0u16.to_le_bytes()), None);
        assert_eq!(
            parse_battery_code(b"ASNYHR3703250525421E232\0\0").as_deref(),
            Some("ASNYHR3703250525421E232")
        );
        assert_eq!(parse_battery_code(b"\0\0").as_deref(), None);
    }

    #[test]
    fn designs_cover_the_supported_playstation_pads() {
        assert_eq!(
            BatteryDesign::for_product(0x0ce6),
            Some(BatteryDesign {
                milliamp_hours: 1_560,
                milliwatt_hours: 5_700
            })
        );
        assert_eq!(
            BatteryDesign::for_product(0x0df2),
            Some(BatteryDesign {
                milliamp_hours: 1_050,
                milliwatt_hours: 3_900
            })
        );
        assert_eq!(BatteryDesign::for_product(0x1234), None);
    }
}
