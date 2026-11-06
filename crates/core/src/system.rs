//! Informations matérielles et charge système, lues sans dépendance externe.
//!
//! Le processeur et la mémoire viennent de `/proc`, les cartes graphiques de
//! `/sys/class/drm` (complétées par `lspci` quand il est présent). Le renderer
//! OpenGL de `glxinfo -B` sert à désigner la carte réellement utilisée.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Informations stables de la machine, relevées une fois au démarrage.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MachineInfo {
    pub hostname: String,
    pub os: String,
    pub kernel: String,
    pub cpu_model: String,
    pub cores: usize,
    pub gpus: Vec<GpuInfo>,
    pub renderer: Option<String>,
}

/// Une carte graphique PCI.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GpuInfo {
    pub name: String,
    pub driver: String,
    pub pci: String,
    /// Carte d'affichage principale (ou celle que le renderer utilise).
    pub active: bool,
}

/// Charge, mémoire vidéo et température de la carte suivie, quand elles sont
/// lisibles.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct GpuLoad {
    pub usage: f32,
    pub vram_used: u64,
    pub vram_total: u64,
    /// Température de la carte, en degrés Celsius, quand la sonde la donne.
    #[serde(default)]
    pub temperature: Option<f32>,
}

/// Relevé complet d'un instant.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SystemInfo {
    pub machine: MachineInfo,
    pub uptime_secs: u64,
    pub cpu_usage: f32,
    /// Charge de chaque cœur, dans l'ordre de `/proc/stat` : la plaque de
    /// performance en fait les barres par cœur de MangoHud.
    #[serde(default)]
    pub cpu_cores: Vec<f32>,
    pub ram_total: u64,
    pub ram_used: u64,
    pub gpu_load: Option<GpuLoad>,
    /// Température du processeur, quand un capteur connu la donne.
    #[serde(default)]
    pub cpu_temp: Option<f32>,
}

impl SystemInfo {
    pub fn ram_percent(&self) -> f32 {
        if self.ram_total == 0 {
            0.0
        } else {
            (self.ram_used as f32 / self.ram_total as f32 * 100.0).clamp(0.0, 100.0)
        }
    }

    pub fn active_gpu(&self) -> Option<&GpuInfo> {
        self.machine.gpus.iter().find(|gpu| gpu.active)
    }
}

/// Occupation d'un système de fichiers, en octets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskUsage {
    pub total: u64,
    /// Octets réellement disponibles pour l'utilisateur, pas seulement libres.
    pub free: u64,
}

impl DiskUsage {
    pub fn free_percent(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.free as f64 / self.total as f64 * 100.0).clamp(0.0, 100.0) as f32
        }
    }

    /// Octets occupés (total moins ce qui reste disponible).
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.free)
    }

    pub fn used_percent(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.used() as f64 / self.total as f64 * 100.0).clamp(0.0, 100.0) as f32
        }
    }
}

/// Espace du système de fichiers qui contient `path`. Un dossier de
/// téléchargements pas encore créé remonte à son premier parent existant.
pub fn disk_usage(path: &Path) -> Option<DiskUsage> {
    use std::os::unix::ffi::OsStrExt;

    let mut probe = path;
    loop {
        if probe.exists() {
            break;
        }
        probe = probe.parent()?;
    }

    let c_path = std::ffi::CString::new(probe.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    let block = if stat.f_frsize > 0 {
        stat.f_frsize as u64
    } else {
        stat.f_bsize as u64
    };
    let total = (stat.f_blocks as u64).saturating_mul(block);
    let free = (stat.f_bavail as u64).saturating_mul(block);
    Some(DiskUsage { total, free })
}

/// Conserve les compteurs précédents pour calculer l'utilisation CPU entre
/// deux relevés.
pub struct SystemSampler {
    machine: MachineInfo,
    previous_total: u64,
    previous_idle: u64,
    gpu_load: Arc<Mutex<Option<GpuLoad>>>,
    gpu_stop: Arc<AtomicBool>,
    /// Compteur des relevés demandés : la sonde cesse d'interroger la carte
    /// quand plus personne n'en veut, plutôt que de la réveiller pour rien.
    gpu_pulse: Arc<std::sync::atomic::AtomicU64>,
}

impl Drop for SystemSampler {
    fn drop(&mut self) {
        self.gpu_stop.store(true, Ordering::Relaxed);
    }
}

impl Default for SystemSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemSampler {
    pub fn new() -> Self {
        let mut sampler = Self {
            machine: machine_info(),
            previous_total: 0,
            previous_idle: 0,
            gpu_load: Arc::new(Mutex::new(None)),
            gpu_stop: Arc::new(AtomicBool::new(false)),
            gpu_pulse: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        sampler.start_gpu_monitor();
        sampler
    }

    /// Interroge la carte graphique en arrière-plan : lancer `nvidia-smi`
    /// toutes les secondes sur le fil de l'interface ferait saccader l'UI.
    fn start_gpu_monitor(&mut self) {
        let probe = GpuProbe::detect(&self.machine);
        self.gpu_stop.store(true, Ordering::Relaxed);
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Mutex::new(None));
        self.gpu_stop = Arc::clone(&stop);
        self.gpu_load = Arc::clone(&shared);
        let Some(probe) = probe else {
            return;
        };
        let pulse = Arc::clone(&self.gpu_pulse);
        let result = thread::Builder::new()
            .name("lapinux-gpu".into())
            .spawn(move || {
                let mut seen = u64::MAX;
                let mut quiet_slices = 0u32;
                while !stop.load(Ordering::Relaxed) {
                    let asked = pulse.load(Ordering::Relaxed);
                    if asked == seen {
                        quiet_slices += 1;
                    } else {
                        seen = asked;
                        quiet_slices = 0;
                    }
                    // Sans demande depuis un quart de minute, la carte n'est
                    // plus interrogée : ni `nvidia-smi`, ni le pilote, ne sont
                    // réveillés quand l'application est rangée.
                    if quiet_slices <= 5 {
                        if let Some(stats) = probe.read() {
                            if let Ok(mut slot) = shared.lock() {
                                *slot = Some(stats);
                            }
                        }
                    }
                    // `nvidia-smi` coûte cher à lancer : une fois par seconde,
                    // deux processus de Lapinux suffisaient à peser quelques
                    // pour cent de processeur au repos. Trois secondes restent
                    // largement assez pour une charge de carte graphique, qui
                    // ne sert qu'au panneau de performances.
                    for _ in 0..30 {
                        if stop.load(Ordering::Relaxed) {
                            return;
                        }
                        thread::sleep(Duration::from_millis(100));
                    }
                }
            });
        if let Err(err) = result {
            tracing::warn!("suivi GPU indisponible : {err}");
        }
    }

    pub fn sample(&mut self) -> SystemInfo {
        self.gpu_pulse.fetch_add(1, Ordering::Relaxed);
        let (total, idle) = cpu_counters().unwrap_or((0, 0));
        let cpu_usage = if self.previous_total == 0 || total <= self.previous_total {
            0.0
        } else {
            let total_delta = total - self.previous_total;
            let idle_delta = idle.saturating_sub(self.previous_idle);
            ((total_delta.saturating_sub(idle_delta)) as f32 / total_delta as f32 * 100.0)
                .clamp(0.0, 100.0)
        };
        self.previous_total = total;
        self.previous_idle = idle;

        let (ram_total, ram_used) = memory().unwrap_or((0, 0));
        let gpu_load = self.gpu_load.lock().ok().and_then(|slot| *slot);
        SystemInfo {
            machine: self.machine.clone(),
            uptime_secs: uptime().unwrap_or(0),
            cpu_usage,
            cpu_cores: cpu_core_loads(),
            ram_total,
            ram_used,
            gpu_load,
            cpu_temp: cpu_temperature(),
        }
    }

    /// Force une nouvelle détection matérielle (après un branchement de GPU).
    pub fn rescan(&mut self) {
        self.machine = machine_info();
        self.previous_total = 0;
        self.previous_idle = 0;
        self.start_gpu_monitor();
    }
}

/// Source de charge GPU disponible sur la machine.
#[derive(Debug)]
enum GpuProbe {
    Nvidia,
    Amd(PathBuf),
}

impl GpuProbe {
    fn detect(machine: &MachineInfo) -> Option<Self> {
        let active = machine.gpus.iter().find(|gpu| gpu.active);
        let identity = active
            .map(|gpu| format!("{} {}", gpu.name, gpu.driver))
            .unwrap_or_default()
            .to_lowercase();
        if identity.contains("nvidia") {
            return nvidia_stats().is_some().then_some(Self::Nvidia);
        }
        if identity.contains("amd") || identity.contains("radeon") || identity.contains("ati") {
            return amd_device(active.map(|gpu| gpu.pci.as_str())).map(Self::Amd);
        }
        // Fabricant inconnu : la première sonde qui répond gagne.
        if nvidia_stats().is_some() {
            return Some(Self::Nvidia);
        }
        amd_device(None).map(Self::Amd)
    }

    fn read(&self) -> Option<GpuLoad> {
        match self {
            Self::Nvidia => nvidia_stats(),
            Self::Amd(device) => amd_stats(device),
        }
    }
}

fn nvidia_stats() -> Option<GpuLoad> {
    // Le champ température est demandé avec les autres mesures ; si un
    // pilote ne le connaît pas, la requête échoue en bloc. Une seconde
    // requête sans ce champ garde alors la charge et la mémoire.
    nvidia_query("utilization.gpu,memory.used,memory.total,temperature.gpu")
        .or_else(|| nvidia_query("utilization.gpu,memory.used,memory.total"))
}

fn nvidia_query(fields: &str) -> Option<GpuLoad> {
    let output = Command::new("nvidia-smi")
        .args([
            &format!("--query-gpu={fields}"),
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_nvidia_stats(&String::from_utf8_lossy(&output.stdout))
}

fn parse_nvidia_stats(stdout: &str) -> Option<GpuLoad> {
    let line = stdout.lines().find(|line| !line.trim().is_empty())?;
    let mut fields = line.split(',').map(str::trim);
    let usage = fields.next()?.parse::<f32>().ok()?.clamp(0.0, 100.0);
    let mib = 1024 * 1024;
    let vram_used = fields.next()?.parse::<u64>().ok()?.saturating_mul(mib);
    let vram_total = fields.next()?.parse::<u64>().ok()?.saturating_mul(mib);
    // Le champ température peut être absent ou `[N/A]` : le reste des
    // mesures reste alors disponible, sans valeur inventée.
    let temperature = fields
        .next()
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|value| (1.0..=125.0).contains(value));
    Some(GpuLoad {
        usage,
        vram_used,
        vram_total,
        temperature,
    })
}

fn amd_device(pci: Option<&str>) -> Option<PathBuf> {
    let mut fallback = None;
    for entry in std::fs::read_dir("/sys/class/drm").ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let device = entry.path().join("device");
        if !device.join("gpu_busy_percent").exists() {
            continue;
        }
        if let Some(pci) = pci {
            let found = std::fs::canonicalize(&device)
                .ok()
                .and_then(|real| real.file_name().map(|name| name.to_string_lossy().to_string()))
                .map(|found| found == pci)
                .unwrap_or(false);
            if found {
                return Some(device);
            }
        }
        fallback.get_or_insert(device);
    }
    fallback
}

fn amd_stats(device: &Path) -> Option<GpuLoad> {
    let read = |name: &str| {
        read_trimmed(&device.join(name).display().to_string())
    };
    let usage = read("gpu_busy_percent")?.parse::<f32>().ok()?.clamp(0.0, 100.0);
    let vram_used = read("mem_info_vram_used")?.parse::<u64>().ok()?;
    let vram_total = read("mem_info_vram_total")?.parse::<u64>().ok()?;
    Some(GpuLoad {
        usage,
        vram_used,
        vram_total,
        temperature: amd_temperature(device),
    })
}

/// Température « edge » de la carte AMD, en degrés Celsius.
fn amd_temperature(device: &Path) -> Option<f32> {
    let entries = std::fs::read_dir(device.join("hwmon")).ok()?;
    for entry in entries.flatten() {
        let path = entry.path().join("temp1_input");
        let Some(raw) = read_trimmed(&path.display().to_string()) else {
            continue;
        };
        let Ok(millidegrees) = raw.parse::<f32>() else {
            continue;
        };
        let value = millidegrees / 1000.0;
        if (1.0..=125.0).contains(&value) {
            return Some(value);
        }
    }
    None
}

/// Température du processeur depuis les capteurs `k10temp` (AMD), `coretemp`
/// (Intel) ou `cpu_thermal` (ARM). Un capteur inconnu est ignoré plutôt que
/// d'afficher la température d'un disque ou de la carte mère.
fn cpu_temperature() -> Option<f32> {
    let entries = std::fs::read_dir("/sys/class/hwmon").ok()?;
    for entry in entries.flatten() {
        let dir = entry.path();
        let name = read_trimmed(&dir.join("name").display().to_string())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let label = read_trimmed(&dir.join("temp1_label").display().to_string())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let known = ["k10temp", "coretemp", "zenpower", "cpu_thermal", "soc_thermal"]
            .iter()
            .any(|sensor| name.contains(sensor))
            || ["package", "tctl", "tdie", "cpu"]
                .iter()
                .any(|sensor| label.contains(sensor));
        if !known {
            continue;
        }
        let Some(raw) = read_trimmed(&dir.join("temp1_input").display().to_string()) else {
            continue;
        };
        let Ok(millidegrees) = raw.parse::<f32>() else {
            continue;
        };
        let value = millidegrees / 1000.0;
        if (1.0..=125.0).contains(&value) {
            return Some(value);
        }
    }
    None
}

fn machine_info() -> MachineInfo {
    let renderer = renderer();
    let mut gpus = gpus();
    if let Some(renderer) = renderer.as_deref() {
        let lower = renderer.to_lowercase();
        let vendor = if lower.contains("nvidia") {
            Some("nvidia")
        } else if lower.contains("intel") {
            Some("intel")
        } else if lower.contains("amd") || lower.contains("radeon") {
            Some("amd")
        } else {
            None
        };
        if let Some(vendor) = vendor {
            for gpu in &mut gpus {
                if gpu.name.to_lowercase().contains(vendor)
                    || gpu.driver.to_lowercase().contains(vendor)
                {
                    gpu.active = true;
                }
            }
        }
    }
    if !gpus.iter().any(|gpu| gpu.active) {
        if let Some(gpu) = gpus.iter_mut().find(|gpu| gpu.driver == "boot_vga") {
            gpu.active = true;
        } else if let Some(gpu) = gpus.first_mut() {
            gpu.active = true;
        }
    }
    MachineInfo {
        hostname: read_trimmed("/proc/sys/kernel/hostname").unwrap_or_default(),
        os: os_name().unwrap_or_else(|| "Linux".into()),
        kernel: read_trimmed("/proc/sys/kernel/osrelease").unwrap_or_default(),
        cpu_model: cpu_model().unwrap_or_else(|| "Processeur inconnu".into()),
        cores: cpu_cores().max(1),
        gpus,
        renderer,
    }
}

fn read_trimmed(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn os_name() -> Option<String> {
    let text = std::fs::read_to_string("/etc/os-release").ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("PRETTY_NAME="))
        .map(|value| value.trim_matches('"').to_string())
}

fn cpu_model() -> Option<String> {
    let text = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("model name"))
        .and_then(|line| line.split_once(':'))
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn cpu_cores() -> usize {
    std::fs::read_to_string("/proc/cpuinfo")
        .map(|text| {
            text.lines()
                .filter(|line| line.starts_with("processor"))
                .count()
        })
        .unwrap_or(0)
}

/// Charge de chaque cœur, en pourcentage, depuis le relevé précédent.
///
/// `/proc/stat` ne donne que des totaux depuis le démarrage : le module garde
/// donc le relevé précédent, comme le fait [`SystemSampler`] pour le total.
fn cpu_core_loads() -> Vec<f32> {
    static PREVIOUS: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());
    let Some(text) = std::fs::read_to_string("/proc/stat").ok() else {
        return Vec::new();
    };
    let mut current = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("cpu") else {
            continue;
        };
        let mut parts = rest.split_whitespace();
        let Some(index) = parts.next() else {
            continue;
        };
        if index.is_empty() || !index.chars().all(|ch| ch.is_ascii_digit()) {
            continue;
        }
        let values: Vec<u64> = parts.filter_map(|value| value.parse().ok()).collect();
        if values.len() < 4 {
            continue;
        }
        let total: u64 = values.iter().sum();
        let idle = values[3] + values.get(4).copied().unwrap_or(0);
        current.push((total, idle));
    }
    let Ok(mut previous) = PREVIOUS.lock() else {
        return Vec::new();
    };
    let loads = current
        .iter()
        .enumerate()
        .map(|(index, (total, idle))| {
            let (previous_total, previous_idle) = previous.get(index).copied().unwrap_or((0, 0));
            if *total <= previous_total {
                0.0
            } else {
                let total_delta = *total - previous_total;
                let idle_delta = idle.saturating_sub(previous_idle);
                ((total_delta.saturating_sub(idle_delta)) as f32 / total_delta as f32 * 100.0)
                    .clamp(0.0, 100.0)
            }
        })
        .collect();
    *previous = current;
    loads
}

fn cpu_counters() -> Option<(u64, u64)> {
    let text = std::fs::read_to_string("/proc/stat").ok()?;
    let line = text.lines().next()?;
    let values: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|value| value.parse().ok())
        .collect();
    if values.len() < 4 {
        return None;
    }
    let total: u64 = values.iter().sum();
    let idle = values[3] + values.get(4).copied().unwrap_or(0);
    Some((total, idle))
}

/// Mémoire en octets : (totale, utilisée).
fn memory() -> Option<(u64, u64)> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let mut total = 0u64;
    let mut available = None;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let key = parts.next()?;
        let value = parts.next()?.parse::<u64>().ok()?;
        let bytes = value.saturating_mul(1024);
        match key {
            "MemTotal:" => total = bytes,
            "MemAvailable:" => available = Some(bytes),
            _ => {}
        }
    }
    let available = available?;
    Some((total, total.saturating_sub(available)))
}

fn uptime() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/uptime").ok()?;
    let seconds: f64 = text.split_whitespace().next()?.parse().ok()?;
    Some(seconds as u64)
}

fn gpus() -> Vec<GpuInfo> {
    let names = lspci_gpus();
    let mut gpus = Vec::new();
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return gpus;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let device = entry.path().join("device");
        let Ok(real) = std::fs::canonicalize(&device) else {
            continue;
        };
        let pci = real
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let driver = std::fs::read_link(device.join("driver"))
            .ok()
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().to_string())
            })
            .unwrap_or_default();
        let boot = std::fs::read_to_string(device.join("boot_vga"))
            .map(|value| value.trim() == "1")
            .unwrap_or(false);
        let fallback = format!(
            "GPU {}:{}",
            read_trimmed(&device.join("vendor").display().to_string()).unwrap_or_default(),
            read_trimmed(&device.join("device").display().to_string()).unwrap_or_default()
        );
        let display = names
            .iter()
            .find(|(bdf, _, _)| bdf.ends_with(&pci) || pci.ends_with(bdf))
            .map(|(_, vendor, name)| format!("{vendor} {name}"))
            .unwrap_or(fallback);
        gpus.push(GpuInfo {
            name: display,
            driver,
            pci,
            active: boot,
        });
    }
    gpus.sort_by(|a, b| b.active.cmp(&a.active).then(a.name.cmp(&b.name)));
    gpus
}

/// `lspci -mm -nn` rend des champs entre guillemets ; on ne garde que les
/// contrôleurs d'affichage.
fn lspci_gpus() -> Vec<(String, String, String)> {
    let Ok(output) = Command::new("lspci").args(["-mm", "-nn"]).output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(parse_lspci_line)
        .collect()
}

fn parse_lspci_line(line: &str) -> Option<(String, String, String)> {
    if !(line.contains("VGA compatible controller")
        || line.contains("3D controller")
        || line.contains("Display controller"))
    {
        return None;
    }
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for character in line.chars() {
        match character {
            '"' => quoted = !quoted,
            ' ' if !quoted => {
                if !current.is_empty() {
                    fields.push(std::mem::take(&mut current));
                }
            }
            other => current.push(other),
        }
    }
    if !current.is_empty() {
        fields.push(current);
    }
    let pci = fields.first()?.clone();
    let vendor = short_vendor(fields.get(2)?);
    let raw_name = fields.get(3)?;
    let name = raw_name
        .split_once('[')
        .and_then(|(_, rest)| rest.split(']').next())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| raw_name.split('[').next().unwrap_or(raw_name).trim())
        .to_string();
    Some((pci, vendor, name))
}

fn short_vendor(vendor: &str) -> String {
    let lower = vendor.to_lowercase();
    if lower.contains("nvidia") {
        "NVIDIA".into()
    } else if lower.contains("amd") || lower.contains("ati") {
        "AMD".into()
    } else if lower.contains("intel") {
        "Intel".into()
    } else if lower.contains("apple") {
        "Apple".into()
    } else {
        vendor.split('[').next().unwrap_or(vendor).trim().to_string()
    }
}

fn renderer() -> Option<String> {
    let output = Command::new("glxinfo").arg("-B").output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("OpenGL renderer string:"))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Vrai si le chemin existe (utile aux tests et au diagnostic).
pub fn exists(path: &str) -> bool {
    Path::new(path).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lspci_keeps_only_display_controllers() {
        let nvidia = r#"01:00.0 "VGA compatible controller [0300]" "NVIDIA Corporation [10de]" "GA104 [GeForce RTX 3070] [2484]" -r a1 "Lenovo [17aa]" "Device [3a4d]""#;
        let parsed = parse_lspci_line(nvidia).unwrap();
        assert_eq!(parsed.0, "01:00.0");
        assert_eq!(parsed.1, "NVIDIA");
        assert_eq!(parsed.2, "GeForce RTX 3070");
        assert!(parse_lspci_line(r#"00:1f.3 "Audio device [0403]" "Intel" "Audio" [1]"#).is_none());
    }

    #[test]
    fn nvidia_measurements_are_parsed() {
        let load = parse_nvidia_stats("35, 2482, 12282, 52\n").unwrap();
        assert!((load.usage - 35.0).abs() < 0.01);
        assert_eq!(load.vram_used, 2482 * 1024 * 1024);
        assert_eq!(load.vram_total, 12282 * 1024 * 1024);
        assert_eq!(load.temperature, Some(52.0));
        // Le champ température est facultatif : un pilote qui ne le rend pas
        // laisse le reste des mesures intact.
        let without_temp = parse_nvidia_stats("35, 2482, 12282\n").unwrap();
        assert_eq!(without_temp.temperature, None);
        assert!(parse_nvidia_stats("[N/A], [N/A], [N/A]").is_none());
    }

    #[test]
    fn usage_is_computed_from_the_delta() {
        let mut sampler = SystemSampler::new();
        sampler.previous_total = 100;
        sampler.previous_idle = 40;
        let (total, idle) = (200u64, 80u64);
        let total_delta = total - sampler.previous_total;
        let idle_delta = idle.saturating_sub(sampler.previous_idle);
        let usage = ((total_delta - idle_delta) as f32 / total_delta as f32 * 100.0) as f32;
        assert!((usage - 60.0).abs() < 0.01);
    }

    #[test]
    fn disk_usage_reads_the_current_filesystem() {
        let usage = disk_usage(Path::new(".")).expect("statvfs disponible");
        assert!(usage.total > 0);
        assert!(usage.free <= usage.total);
        assert!((0.0..=100.0).contains(&usage.free_percent()));
        assert!((0.0..=100.0).contains(&usage.used_percent()));
        assert!(usage.used() <= usage.total);
    }

    #[test]
    fn disk_usage_accepts_a_not_yet_created_folder() {
        let usage = disk_usage(Path::new("/tmp/lapinux-dossier-encore-absent/sous-dossier"))
            .expect("remonte au parent existant");
        assert!(usage.total > 0);
    }

    #[test]
    fn machine_has_a_name_and_some_memory() {
        let mut sampler = SystemSampler::new();
        let info = sampler.sample();
        assert!(!info.machine.cpu_model.is_empty());
        assert!(info.machine.cores >= 1);
        assert!(info.ram_total > 0);
    }
}
