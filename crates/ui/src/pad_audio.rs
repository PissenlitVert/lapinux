//! Audio de la manette : sortie du haut-parleur, prise casque et niveau du
//! microphone. Tout passe par PipeWire/PulseAudio en ligne de commande : le
//! projet n'embarque pas de client audio natif.

use lapinux_core::model::{DeviceInfo, DeviceLink};
use serde_json::Value;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadsetState {
    Plugged,
    Unplugged,
    /// Le Bluetooth ne transporte pas l'état de la prise jack.
    Bluetooth,
    Unknown,
}

pub struct AudioRoute {
    pub sink: Option<String>,
    pub source: Option<String>,
    pub headset: HeadsetState,
    /// Volume du haut-parleur de la manette, lu sur la sortie du système.
    pub volume: Option<u8>,
}

/// État audio d'une manette, avec détection espacée et jauge de micro.
pub struct PadAudio {
    pub key: String,
    route: AudioRoute,
    mic: Option<MicMeter>,
    monitor: Option<Monitor>,
    mic_enabled: bool,
    last_scan: Instant,
}

impl PadAudio {
    pub fn new(key: String, device: &DeviceInfo, mic_enabled: bool) -> Self {
        let route = detect_route(device);
        let mic = if mic_enabled {
            route.source.as_deref().and_then(MicMeter::start)
        } else {
            None
        };
        Self {
            key,
            route,
            mic,
            monitor: None,
            mic_enabled,
            last_scan: Instant::now(),
        }
    }

    /// Rafraîchit la détection quand la manette est branchée ou qu'un profil
    /// audio change. L'appel `pactl` reste espacé pour ne pas bloquer l'UI.
    pub fn refresh(&mut self, device: &DeviceInfo) {
        if self.last_scan.elapsed() < Duration::from_secs(4) {
            return;
        }
        self.last_scan = Instant::now();
        let route = detect_route(device);
        if route.source != self.route.source {
            // Une source qui change invalide le flux d'écoute : l'interface
            // le relancera sur la nouvelle entrée après ce rafraîchissement.
            self.stop_monitor();
            self.mic = if self.mic_enabled {
                route.source.as_deref().and_then(MicMeter::start)
            } else {
                None
            };
        }
        self.route = route;
    }

    pub fn sink(&self) -> Option<&str> {
        self.route.sink.as_deref()
    }

    pub fn source(&self) -> Option<&str> {
        self.route.source.as_deref()
    }

    pub fn headset(&self) -> HeadsetState {
        self.route.headset
    }

    /// Volume actuel du haut-parleur de la manette (0 à 100), quand la sortie
    /// existe. Il est lu sur la destination, pas estimé.
    pub fn speaker_volume(&self) -> Option<u8> {
        self.route.volume
    }

    /// Règle le volume du haut-parleur de la manette, et le rétablit s'il
    /// était coupé. La destination est propre à la manette : les autres
    /// sorties du système ne bougent pas.
    pub fn set_speaker_volume(&self, percent: u8) -> Result<(), String> {
        let sink = self
            .route
            .sink
            .as_deref()
            .ok_or_else(|| lapinux_core::i18n::t("pad.no_output").to_string())?;
        let _ = Command::new("pactl")
            .args(["set-sink-mute", sink, "0"])
            .env("LC_ALL", "C")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let output = Command::new("pactl")
            .args(["set-sink-volume", sink, &format!("{}%", percent.min(100))])
            .env("LC_ALL", "C")
            .output()
            .map_err(|err| lapinux_core::i18n::t_args("pad.speaker_volume_error", &[("error", &err.to_string())]))?;
        if output.status.success() {
            Ok(())
        } else {
            let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(if reason.is_empty() {
                lapinux_core::i18n::t("pad.volume_failed").to_string()
            } else {
                reason
            })
        }
    }

    pub fn mic_level(&self) -> f32 {
        self.mic
            .as_ref()
            .map(|mic| mic.level())
            .unwrap_or(0.0)
    }

    /// Ouvre ou coupe l'écoute directe du micro. `parec` capte la source et
    /// `pacat` la joue sur la sortie système, sans client audio natif.
    pub fn sync_monitor(&mut self, enabled: bool) -> Result<(), String> {
        if !enabled {
            self.stop_monitor();
            return Ok(());
        }
        let source = self.route.source.as_deref().ok_or_else(|| {
            lapinux_core::i18n::t("pad.no_mic")
                .to_string()
        })?;
        if self.monitor.is_some() {
            return Ok(());
        }
        // Évite de renvoyer le micro dans le haut-parleur de la manette
        // quand PipeWire l'a élue sortie par défaut au branchement USB.
        let sink = playback_target(self.route.sink.as_deref());
        self.monitor = Some(Monitor::start(source, sink.as_deref())?);
        Ok(())
    }

    fn stop_monitor(&mut self) {
        self.monitor = None;
    }

    /// Joue une petite note sur le haut-parleur de la manette. Le niveau est
    /// celui de la destination (curseur Volume), pas un volume de flux caché :
    /// le son peut donc monter jusqu'au maximum du haut-parleur.
    pub fn play_tone(&self) -> Result<(), String> {
        let sink = self
            .route
            .sink
            .as_deref()
            .ok_or_else(|| lapinux_core::i18n::t("pad.no_output").to_string())?;
        let path = tone_path();
        write_tone(&path).map_err(|err| lapinux_core::i18n::t_args("pad.tone_error", &[("error", &err.to_string())]))?;
        let mut child = Command::new("pw-play")
            .arg("--target")
            .arg(sink)
            .arg("--volume")
            .arg("1.00")
            .arg(&path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .or_else(|_| {
                Command::new("paplay")
                    .arg(format!("--device={sink}"))
                    .arg("--volume=65536")
                    .arg(&path)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
            })
            .map_err(|err| format!("lecteur audio indisponible : {err}"))?;
        // Le processus est récolté hors du fil d'affichage.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }

    /// Retours haptiques HD en USB : la manette expose une carte son quatre
    /// canaux, dont les deux arrière alimentent les bobines. On joue donc le
    /// son des bobines sur ces canaux, haut-parleur muet.
    pub fn play_haptics(
        &self,
        left: f32,
        right: f32,
        frequency_hz: f32,
        duration_ms: u16,
    ) -> Result<(), String> {
        let sink = self
            .route
            .sink
            .as_deref()
            .ok_or_else(|| lapinux_core::i18n::t("pad.no_card").to_string())?;
        let path = haptics_path();
        write_haptics(&path, left, right, frequency_hz, duration_ms)
            .map_err(|err| lapinux_core::i18n::t_args("pad.haptics_error", &[("error", &err.to_string())]))?;
        let mut child = Command::new("pw-play")
            .args([
                "--target",
                sink,
                "--channels",
                "4",
                "--channel-map",
                "FL,FR,RL,RR",
                "--volume",
                "1.00",
            ])
            .arg(&path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| format!("lecteur audio indisponible : {err}"))?;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}

/// Traduit un volume 0..1 pour les deux lecteurs possibles : pw-play prend un
/// facteur linéaire, paplay la même valeur en 16 bits (0..65536).
/// Volume moyen d'une destination, en pourcentage. `pactl` donne une valeur
/// par canal ; le haut-parleur d'une manette a le même niveau partout.
fn sink_volume_percent(sink: &Value) -> Option<u8> {
    let channels = sink.get("volume")?.as_object()?;
    let mut total = 0f64;
    let mut count = 0f64;
    for channel in channels.values() {
        if let Some(value) = channel.get("value").and_then(Value::as_u64) {
            total += value as f64;
            count += 1.0;
        }
    }
    (count > 0.0).then(|| ((total / count) / 65_536.0 * 100.0).round().clamp(0.0, 100.0) as u8)
}

fn detect_route(device: &DeviceInfo) -> AudioRoute {
    let cards = pactl_json(&["-f", "json", "list", "cards"]);
    let sinks = pactl_json(&["-f", "json", "list", "short", "sinks"]);
    let sink_details = pactl_json(&["-f", "json", "list", "sinks"]);
    let sources = pactl_json(&["-f", "json", "list", "short", "sources"]);

    let card = cards
        .iter()
        .find(|card| card_matches_controller(card, device));
    let sink = sinks
        .iter()
        .filter_map(|sink| sink.get("name").and_then(Value::as_str))
        .find(|name| matches_controller(name, device))
        .map(str::to_string);
    let volume = sink.as_deref().and_then(|name| {
        sink_details
            .iter()
            .find(|sink| sink.get("name").and_then(Value::as_str) == Some(name))
            .and_then(sink_volume_percent)
    });
    let source = sources
        .iter()
        .filter_map(|source| source.get("name").and_then(Value::as_str))
        .find(|name| is_capture_source(name) && matches_controller(name, device))
        .map(str::to_string);

    let headset = match (device.connection, card) {
        (DeviceLink::Bluetooth, _) => HeadsetState::Bluetooth,
        (_, Some(card)) => headset_from_card(card),
        (_, None) => HeadsetState::Unknown,
    };

    AudioRoute {
        sink,
        source,
        headset,
        volume,
    }
}

fn pactl_json(args: &[&str]) -> Vec<Value> {
    let Ok(output) = Command::new("pactl")
        .args(args)
        .env("LC_ALL", "C")
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    serde_json::from_slice(&output.stdout).unwrap_or_default()
}

fn card_matches_controller(card: &Value, device: &DeviceInfo) -> bool {
    let mut text = card
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if let Some(properties) = card.get("properties").and_then(Value::as_object) {
        for key in [
            "device.description",
            "device.name",
            "device.string",
            "api.alsa.card.longname",
            "api.bluez5.address",
        ] {
            if let Some(value) = properties.get(key).and_then(Value::as_str) {
                text.push(' ');
                text.push_str(value);
            }
        }
    }
    matches_controller(&text, device)
}

/// Une source `.monitor` est la boucle d'une sortie (le haut-parleur de la
/// manette), pas son microphone.
fn is_capture_source(name: &str) -> bool {
    !name.ends_with(".monitor")
}

fn matches_controller(name: &str, device: &DeviceInfo) -> bool {
    let name = name.to_ascii_lowercase();
    if name.contains("dualsense") || name.contains("dualshock") {
        return true;
    }
    if let Some(address) = &device.address {
        let address = address.to_ascii_lowercase();
        if name.contains(&address)
            || name.contains(&address.replace(':', "_"))
            || name.contains(&address.replace(':', "-"))
        {
            return true;
        }
    }
    name.contains("playstation") && name.contains("controller")
}

fn headset_from_card(card: &Value) -> HeadsetState {
    let Some(ports) = card.get("ports").and_then(Value::as_object) else {
        return HeadsetState::Unknown;
    };
    let mut unplugged = false;
    for (name, port) in ports {
        let name = name.to_ascii_lowercase();
        let description = port
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        let kind = port
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        let headphones = name.contains("headphone")
            || name.contains("headset")
            || description.contains("casque")
            || description.contains("headphone")
            || kind == "headphones"
            || kind == "headset";
        if !headphones {
            continue;
        }
        match port.get("availability").and_then(Value::as_str) {
            Some("available") => return HeadsetState::Plugged,
            Some("not available") => unplugged = true,
            _ => {}
        }
    }
    if unplugged {
        HeadsetState::Unplugged
    } else {
        HeadsetState::Unknown
    }
}

/// Sortie d'écoute : la sortie par défaut, sauf si c'est le haut-parleur de
/// la manette (cela bouclerait le micro dans son propre haut-parleur).
fn playback_target(controller_sink: Option<&str>) -> Option<String> {
    let default = Command::new("pactl")
        .args(["get-default-sink"])
        .env("LC_ALL", "C")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    if let Some(default) = default {
        if Some(default.as_str()) != controller_sink {
            return Some(default);
        }
    }
    let sinks = pactl_json(&["-f", "json", "list", "short", "sinks"]);
    sinks
        .iter()
        .filter_map(|sink| sink.get("name").and_then(Value::as_str))
        .find(|name| Some(*name) != controller_sink)
        .map(str::to_string)
}

/// Écoute directe du micro : `parec` capture la source et `pacat` la joue
/// sur la sortie système. Deux processus plutôt qu'un client audio natif,
/// comme le reste de ce module.
struct Monitor {
    capture: Child,
    playback: Child,
}

impl Monitor {
    fn start(source: &str, sink: Option<&str>) -> Result<Self, String> {
        let mut capture = Command::new("parec")
            .args([
                "--device",
                source,
                "--format=float32le",
                "--rate=48000",
                "--channels=1",
                "--latency-msec=40",
                "--raw",
            ])
            .env("LC_ALL", "C")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| format!("capture micro : {err}"))?;
        let Some(stdout) = capture.stdout.take() else {
            let _ = capture.kill();
            let _ = capture.wait();
            return Err("flux micro indisponible".to_string());
        };
        let mut playback = Command::new("pacat");
        playback
            .args([
                "--playback",
                "--raw",
                "--format=float32le",
                "--rate=48000",
                "--channels=1",
                "--latency-msec=40",
            ])
            .env("LC_ALL", "C");
        if let Some(sink) = sink {
            playback.arg(format!("--device={sink}"));
        }
        let playback = match playback
            .stdin(Stdio::from(stdout))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(err) => {
                let _ = capture.kill();
                let _ = capture.wait();
                return Err(format!("lecture micro : {err}"));
            }
        };
        Ok(Self { capture, playback })
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        let _ = self.capture.kill();
        let _ = self.capture.wait();
        let _ = self.playback.kill();
        let _ = self.playback.wait();
    }
}

/// Microphone capté par `parec`, réduit à un niveau RMS lissé.
struct MicMeter {
    level: Arc<AtomicU32>,
    child: Child,
}

impl MicMeter {
    fn start(source: &str) -> Option<Self> {
        let mut child = Command::new("parec")
            .args([
                "--device",
                source,
                "--format=float32le",
                "--rate=8000",
                "--channels=1",
                "--latency-msec=20",
                "--raw",
            ])
            .env("LC_ALL", "C")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut stdout = child.stdout.take()?;
        let level = Arc::new(AtomicU32::new(0));
        let thread_level = Arc::clone(&level);
        std::thread::spawn(move || {
            let mut buffer = [0u8; 640];
            let mut smooth = 0.0_f32;
            loop {
                let read = match stdout.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => read,
                    Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                };
                let mut sum = 0.0_f64;
                let mut count = 0usize;
                for sample in buffer[..read].chunks_exact(4) {
                    let value =
                        f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]);
                    sum += (value * value) as f64;
                    count += 1;
                }
                if count == 0 {
                    continue;
                }
                let rms = (sum / count as f64).sqrt() as f32;
                let db = 20.0 * rms.max(1e-7).log10();
                let target = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
                smooth = if target > smooth {
                    smooth * 0.55 + target * 0.45
                } else {
                    (smooth - 0.02).max(target)
                };
                thread_level.store((smooth * 1000.0) as u32, Ordering::Relaxed);
            }
        });
        Some(Self { level, child })
    }

    fn level(&self) -> f32 {
        self.level.load(Ordering::Relaxed) as f32 / 1000.0
    }
}

impl Drop for MicMeter {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn tone_path() -> std::path::PathBuf {
    std::env::temp_dir().join("lapinux-controller-speaker.wav")
}

fn haptics_path() -> std::path::PathBuf {
    std::env::temp_dir().join("lapinux-controller-haptics.wav")
}

/// Sinus 660 Hz clair, avec fondu d'entrée et de sortie. L'amplitude laisse
/// de la marge : c'est le volume de la destination qui décide du niveau.
fn write_tone(path: &std::path::Path) -> std::io::Result<()> {
    const RATE: u32 = 48_000;
    const SECONDS: f32 = 0.8;
    let samples = (RATE as f32 * SECONDS) as u32;
    let mut pcm = Vec::with_capacity(samples as usize * 2);
    for index in 0..samples {
        let t = index as f32 / RATE as f32;
        let fade = (t / 0.04).min(1.0) * ((SECONDS - t) / 0.12).min(1.0).max(0.0);
        let envelope = fade * (-1.2 * t).exp();
        let sample = (t * 660.0 * std::f32::consts::TAU).sin() * envelope * 0.55;
        pcm.extend_from_slice(&((sample * i16::MAX as f32) as i16).to_le_bytes());
    }
    write_wav(path, RATE, 1, &pcm)
}

/// WAV PCM 16 bits, petit-boutiste. Le nombre de canaux décide du reste :
/// une note est mono, les retours HD utilisent les quatre canaux du câble.
fn write_wav(
    path: &std::path::Path,
    rate: u32,
    channels: u16,
    pcm: &[u8],
) -> std::io::Result<()> {
    let block_align = channels * 2;
    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&((36 + pcm.len()) as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * block_align as u32).to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    wav.extend_from_slice(pcm);
    std::fs::write(path, wav)
}

/// WAV quatre canaux pour les bobines HD en USB : haut-parleur muet sur
/// `FL`/`FR`, son de bobine sur `RL`/`RR`, qui sont les deux canaux arrière
/// de la carte son de la DualSense.
fn write_haptics(
    path: &std::path::Path,
    left: f32,
    right: f32,
    frequency_hz: f32,
    duration_ms: u16,
) -> std::io::Result<()> {
    const RATE: u32 = 48_000;
    let left = left.clamp(0.0, 1.0);
    let right = right.clamp(0.0, 1.0);
    let frequency_hz = frequency_hz.clamp(20.0, 600.0);
    let seconds = duration_ms.clamp(100, 5000) as f32 / 1000.0;
    let samples = (RATE as f32 * seconds) as u32;
    let fade = (RATE as f32 * 0.02) as u32;
    let step = std::f32::consts::TAU * frequency_hz / RATE as f32;
    let mut pcm = Vec::with_capacity(samples as usize * 8);
    let mut phase = 0f32;
    for index in 0..samples {
        let attack = (index as f32 / fade as f32).min(1.0);
        let release = ((samples.saturating_sub(index)) as f32 / fade as f32).min(1.0);
        let gain = attack.min(release).clamp(0.0, 1.0);
        let wave = phase.sin() * gain;
        phase += step;
        let bobine_gauche = (wave * left * 0.9 * i16::MAX as f32) as i16;
        let bobine_droite = (wave * right * 0.9 * i16::MAX as f32) as i16;
        for sample in [0i16, 0, bobine_gauche, bobine_droite] {
            pcm.extend_from_slice(&sample.to_le_bytes());
        }
    }
    write_wav(path, RATE, 4, &pcm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn device(address: Option<&str>) -> DeviceInfo {
        DeviceInfo {
            path: PathBuf::new(),
            name: "DualSense Wireless Controller".into(),
            vendor: 0x054c,
            product: 0x0ce6,
            gamepad: true,
            buttons: 13,
            button_names: Vec::new(),
            axes: Vec::new(),
            force_feedback: false,
            touchpad: false,
            connection: if address.is_some() {
                DeviceLink::Bluetooth
            } else {
                DeviceLink::Usb
            },
            battery: None,
            battery_status: None,
            address: address.map(str::to_string),
            connected: true,
            steam_bridge: false,
            udev: Default::default(),
        }
    }

    #[test]
    fn bluetooth_names_match_the_controller_address() {
        let pad = device(Some("44:46:48:2E:38:09"));
        assert!(matches_controller(
            "bluez_output.44:46:48:2E:38:09.1",
            &pad
        ));
        assert!(matches_controller("bluez_card.44_46_48_2E_38_09", &pad));
        assert!(!matches_controller("bluez_output.CB_F5_35_81_15_40.1", &pad));
    }

    #[test]
    fn speaker_monitor_is_not_the_microphone() {
        assert!(!is_capture_source(
            "alsa_output.usb-Sony_DualSense-00.analog-stereo.monitor"
        ));
        assert!(is_capture_source(
            "alsa_input.usb-Sony_DualSense-00.mono-fallback"
        ));
    }

    #[test]
    fn usb_names_match_the_model() {
        let pad = device(None);
        assert!(matches_controller(
            "alsa_output.usb-Sony_Interactive_Entertainment_DualSense_Wireless_Controller-00.analog-stereo",
            &pad
        ));
        assert!(!matches_controller("alsa_card.pci-0000_0a_00.4", &pad));
    }

    #[test]
    fn headset_port_availability_is_read() {
        let card = serde_json::json!({
            "ports": {
                "analog-output-headphones": {
                    "description": "Casque audio",
                    "type": "Headphones",
                    "availability": "available"
                }
            }
        });
        assert_eq!(headset_from_card(&card), HeadsetState::Plugged);

        let card = serde_json::json!({
            "ports": {
                "analog-output-headphones": {
                    "description": "Casque audio",
                    "type": "Headphones",
                    "availability": "not available"
                }
            }
        });
        assert_eq!(headset_from_card(&card), HeadsetState::Unplugged);
    }

    #[test]
    fn sink_volume_is_read_from_pactl_json() {
        let sink = serde_json::json!({
            "name": "alsa_output.usb-Sony_DualSense",
            "volume": {
                "front-left": { "value": 32768 },
                "front-right": { "value": 32768 },
                "rear-left": { "value": 32768 },
                "rear-right": { "value": 32768 }
            }
        });
        assert_eq!(sink_volume_percent(&sink), Some(50));
        assert_eq!(sink_volume_percent(&serde_json::json!({})), None);
    }

    #[test]
    fn haptics_wav_keeps_the_speaker_silent() {
        let dir = std::env::temp_dir().join("lapinux-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("haptics.wav");
        write_haptics(&path, 1.0, 1.0, 150.0, 300).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(u16::from_le_bytes([bytes[22], bytes[23]]), 4);
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 48_000);
        let data = &bytes[44..];
        let frames = data.len() / 8;
        assert!(frames > 10_000, "durée trop courte : {frames} images");
        let mut front_peak = 0i16;
        let mut rear_peak = 0i16;
        for frame in 0..frames {
            let sample = |channel: usize| {
                let offset = frame * 8 + channel * 2;
                i16::from_le_bytes([data[offset], data[offset + 1]])
            };
            front_peak = front_peak.max(sample(0).abs()).max(sample(1).abs());
            rear_peak = rear_peak.max(sample(2).abs()).max(sample(3).abs());
        }
        assert_eq!(front_peak, 0, "le haut-parleur reste muet");
        assert!(rear_peak > 20_000, "les bobines reçoivent le son : {rear_peak}");
    }

    #[test]
    fn tone_is_a_valid_mono_wav() {
        let dir = std::env::temp_dir().join("lapinux-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tone.wav");
        write_tone(&path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(u16::from_le_bytes([bytes[22], bytes[23]]), 1);
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 48_000);
        assert!(bytes.len() > 44);
    }
}
