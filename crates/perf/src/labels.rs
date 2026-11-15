//! Noms courts du processeur et de la carte graphique pour le HUD natif.

use std::process::Command;
use std::sync::OnceLock;

pub fn cpu_label() -> Option<String> {
    static CPU: OnceLock<Option<String>> = OnceLock::new();
    CPU.get_or_init(detect_cpu_label).clone()
}

fn detect_cpu_label() -> Option<String> {
    let text = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    let model = text.lines().find_map(|line| {
        line.strip_prefix("model name")
            .and_then(|rest| rest.split_once(':'))
            .map(|(_, value)| value.trim())
    })?;
    short_cpu_name(model)
}

/// Nom public de la carte graphique, raccourci pour le HUD. Le résultat est
/// mis en cache : le modèle ne change pas pendant la session.
pub fn gpu_label() -> Option<String> {
    static GPU: OnceLock<Option<String>> = OnceLock::new();
    GPU.get_or_init(detect_gpu_label).clone()
}

fn detect_gpu_label() -> Option<String> {
    // NVIDIA expose un nom marketing propre, sans dépendre de pciutils.
    if let Ok(output) = Command::new("nvidia-smi")
        .args(["--query-gpu=name", "--format=csv,noheader"])
        .output()
    {
        if output.status.success() {
            if let Some(line) = String::from_utf8_lossy(&output.stdout).lines().next() {
                if let Some(name) = short_gpu_name(line) {
                    return Some(name);
                }
            }
        }
    }
    // AMD et Intel : `lspci -mm` fournit le nom du modèle entre guillemets.
    let output = Command::new("lspci").arg("-mm").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let class = line.to_ascii_lowercase();
        if !(class.contains("vga compatible controller")
            || class.contains("3d controller")
            || class.contains("display controller"))
        {
            continue;
        }
        // `lspci -mm` : "classe" "vendeur" "modèle".
        let quoted: Vec<&str> = line.split('"').collect();
        if let Some(device) = quoted.get(5) {
            if let Some(name) = short_gpu_name(device) {
                return Some(name);
            }
        }
    }
    None
}

fn short_cpu_name(raw: &str) -> Option<String> {
    let mut name = raw
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>();
    // « … @ 3.60GHz » ne sert à rien dans le HUD.
    if let Some((head, _)) = name.split_once('@') {
        name = head.to_string();
    }
    name = name.trim().to_string();
    for suffix in [" CPU", " Processor"] {
        if let Some(stripped) = name.strip_suffix(suffix) {
            name = stripped.to_string();
        }
    }
    // « 8-Core » décrit le nombre de cœurs, pas le modèle.
    let mut words: Vec<&str> = name.split_whitespace().collect();
    if words.last().is_some_and(|word| word.ends_with("-Core")) {
        words.pop();
    }
    name = words.join(" ");
    for prefix in [
        "AMD ",
        "Intel(R) ",
        "Intel ",
        "Genuine Intel(R) ",
        "Genuine Intel ",
    ] {
        if let Some(stripped) = name.strip_prefix(prefix) {
            name = stripped.to_string();
        }
    }
    name = name.replace("(R)", "").replace("(TM)", "");
    name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        None
    } else {
        Some(name.chars().take(28).collect())
    }
}

fn short_gpu_name(raw: &str) -> Option<String> {
    let mut name = raw.trim().to_string();
    // « AD104 [GeForce RTX 4070] » : le nom commercial est entre crochets.
    if let Some(open) = name.rfind('[') {
        if let Some(close) = name[open..].find(']') {
            name = name[open + 1..open + close].to_string();
        }
    }
    for prefix in [
        "NVIDIA ",
        "GeForce ",
        "AMD ",
        "ATI ",
        "Advanced Micro Devices, Inc. ",
        "Intel(R) ",
        "Intel ",
    ] {
        if let Some(stripped) = name.strip_prefix(prefix) {
            name = stripped.to_string();
        }
    }
    name = name
        .replace("(R)", "")
        .replace("(TM)", "")
        .replace(" Graphics", "");
    name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        None
    } else {
        Some(name.chars().take(28).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_names_are_shortened_for_the_hud() {
        assert_eq!(
            short_cpu_name("AMD Ryzen 7 7800X3D 8-Core Processor"),
            Some("Ryzen 7 7800X3D".into())
        );
        assert_eq!(
            short_cpu_name("Intel(R) Core(TM) i5-12400 @ 2.50GHz"),
            Some("Core i5-12400".into())
        );
    }

    #[test]
    fn gpu_names_keep_the_marketing_model() {
        assert_eq!(
            short_gpu_name("AD104 [GeForce RTX 4070]"),
            Some("RTX 4070".into())
        );
        assert_eq!(
            short_gpu_name("NVIDIA GeForce RTX 3060 Ti"),
            Some("RTX 3060 Ti".into())
        );
    }
}
