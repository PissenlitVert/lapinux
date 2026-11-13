//! Préparation et lancement des jeux Steam.

use super::*;

/// Le pont natif vise l'ABI x86-64 ; il ne doit jamais être injecté dans un
/// exécutable ELF d'une autre architecture. Les scripts de lancement restent
/// possibles, mais leur processus final n'est pas détectable ici.
fn elf_arch(path: &std::path::Path) -> std::result::Result<Option<(u8, u16)>, String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| format!("{} : {e}", path.display()))?;
    let mut magic = [0; 4];
    if file.read_exact(&mut magic).is_err() || magic != *b"\x7fELF" {
        return Ok(None);
    }
    let mut rest = [0; 16];
    file.read_exact(&mut rest)
        .map_err(|_| format!("ELF tronqué : {}", path.display()))?;
    let machine = match rest[1] {
        1 => u16::from_le_bytes([rest[14], rest[15]]),
        2 => u16::from_be_bytes([rest[14], rest[15]]),
        _ => return Err(format!("ELF invalide : {}", path.display())),
    };
    Ok(Some((rest[0], machine)))
}

/// Architecture du processus Vulkan probable du jeu : ELF pour un jeu natif,
/// en-tête PE pour un jeu Windows lancé par Proton. Un lanceur opaque renvoie
/// `None`, et on garde alors le runtime 64 bits par défaut.
fn game_bits(path: &std::path::Path) -> Option<u32> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let mut head = [0u8; 64];
    let read = file.read(&mut head).ok()?;
    if read >= 20 && &head[..4] == b"\x7fELF" {
        return match head[4] {
            1 => Some(32),
            2 => Some(64),
            _ => None,
        };
    }
    if read < 64 || &head[..2] != b"MZ" {
        return None;
    }
    let offset = u32::from_le_bytes(head[0x3c..0x40].try_into().ok()?) as u64;
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut pe = [0u8; 6];
    file.read_exact(&mut pe).ok()?;
    if &pe[..4] != b"PE\0\0" {
        return None;
    }
    match u16::from_le_bytes([pe[4], pe[5]]) {
        0x014c => Some(32),
        0x8664 | 0xaa64 => Some(64),
        _ => None,
    }
}

/// Contrôles structurels du pont sans charger du code dans le daemon. Ils ne
/// garantissent pas que le chargeur pourra résoudre tous les symboles.
fn valid_native_shim(path: &std::path::Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    fn u16le(b: &[u8]) -> u16 {
        u16::from_le_bytes(b.try_into().unwrap())
    }
    fn u32le(b: &[u8]) -> u32 {
        u32::from_le_bytes(b.try_into().unwrap())
    }
    fn u64le(b: &[u8]) -> u64 {
        u64::from_le_bytes(b.try_into().unwrap())
    }
    fn read_at(file: &mut std::fs::File, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(buf)
    }
    let result = (|| -> Option<bool> {
        let mut file = std::fs::File::open(path).ok()?;
        let len = file.metadata().ok()?.len();
        let mut h = [0u8; 64];
        read_at(&mut file, 0, &mut h).ok()?;
        if &h[..7] != b"\x7fELF\x02\x01\x01"
            || u16le(&h[16..18]) != 3
            || u16le(&h[18..20]) != 62
            || u32le(&h[20..24]) != 1
            || u16le(&h[52..54]) != 64
            || u16le(&h[54..56]) != 56
        {
            return Some(false);
        }
        let phoff = u64le(&h[32..40]);
        let count = u16le(&h[56..58]) as u64;
        if count == 0 || count > 4096 || phoff.checked_add(count.checked_mul(56)?)? > len {
            return Some(false);
        }
        let mut loads = Vec::new();
        let mut dynamic = None;
        let mut interpreter = false;
        for index in 0..count {
            let mut ph = [0u8; 56];
            read_at(&mut file, phoff + index * 56, &mut ph).ok()?;
            let kind = u32le(&ph[..4]);
            let offset = u64le(&ph[8..16]);
            let addr = u64le(&ph[16..24]);
            let size = u64le(&ph[32..40]);
            if offset.checked_add(size)? > len {
                return Some(false);
            }
            if kind == 1 {
                loads.push((addr, offset, size));
            }
            if kind == 2 {
                dynamic = Some((offset, size));
            }
            if kind == 3 {
                interpreter = true;
            }
        }
        // Un exécutable PIE est aussi ET_DYN, mais ne peut pas être préchargé.
        if interpreter {
            return Some(false);
        }
        let (offset, size) = dynamic?;
        if size == 0 || size > 1024 * 1024 || size % 16 != 0 {
            return Some(false);
        }
        let mut table = vec![0; size as usize];
        read_at(&mut file, offset, &mut table).ok()?;
        let (mut strtab, mut strsz, mut soname, mut terminated) = (None, None, None, false);
        for entry in table.chunks_exact(16) {
            let tag = u64le(&entry[..8]);
            let value = u64le(&entry[8..]);
            match tag {
                0 => {
                    terminated = true;
                    break;
                }
                5 => strtab = Some(value),
                10 => strsz = Some(value),
                14 => soname = Some(value),
                _ => {}
            }
        }
        if !terminated {
            return Some(false);
        }
        let (strtab, strsz, soname) = (strtab?, strsz?, soname?);
        let expected = b"libsteam_api.so\0";
        if soname.checked_add(expected.len() as u64)? > strsz {
            return Some(false);
        }
        let address = strtab.checked_add(soname)?;
        let offset = loads.iter().find_map(|&(addr, offset, size)| {
            let delta = address.checked_sub(addr)?;
            if delta.checked_add(expected.len() as u64)? <= size {
                offset.checked_add(delta)
            } else {
                None
            }
        })?;
        let mut name = [0u8; 16];
        read_at(&mut file, offset, &mut name).ok()?;
        Some(&name == expected)
    })();
    result.unwrap_or(false)
}

fn native_shim_path(
    preference: Option<bool>,
    explicit: Option<std::path::PathBuf>,
    data_dir: &std::path::Path,
    binary_dir: Option<&std::path::Path>,
    exe: &std::path::Path,
) -> std::result::Result<Option<std::path::PathBuf>, String> {
    // Les anciennes entrées natifs n'avaient pas d'interrupteur. Une valeur
    // non renseignée reste inactive : UI et daemon ont le même défaut.
    if !preference.unwrap_or(false) {
        return Ok(None);
    }
    let shim = match explicit {
        Some(path) => path,
        None => [Some(data_dir.join("libsteam_api.so")), binary_dir.map(|dir| dir.join("libsteam_api.so"))]
            .into_iter().flatten().find(|path| path.is_file())
            .ok_or_else(|| "pont Steamworks natif introuvable : compiler libsteam_api.so et le placer à côté de lapinux ou dans le dossier de données".to_string())?,
    };
    let shim = std::fs::canonicalize(&shim)
        .map_err(|error| format!("pont Steamworks natif {} : {error}", shim.display()))?;
    if !shim.is_file() || shim.to_str().is_none() {
        return Err(format!(
            "pont Steamworks natif {} : fichier régulier avec chemin UTF-8 requis",
            shim.display()
        ));
    }
    if shim
        .to_string_lossy()
        .chars()
        .any(|c| c == ':' || c.is_whitespace())
    {
        return Err("le chemin du pont Steamworks ne peut contenir ni espace ni deux-points pour LD_PRELOAD".into());
    }
    if !valid_native_shim(&shim) {
        return Err(format!("pont Steamworks {} incompatible ou corrompu : bibliothèque ELF x86-64 avec SONAME libsteam_api.so requise", shim.display()));
    }
    if let Some(arch) = elf_arch(exe)? {
        if arch != (2, 62) {
            return Err(
                "le pont Steamworks x86-64 est incompatible avec l'architecture du jeu".into(),
            );
        }
    } else {
        tracing::warn!("pont Steamworks : architecture finale du jeu inconnue (lanceur non ELF)");
    }
    Ok(Some(shim))
}

/// Cadence réelle à demander au jeu quand la couche ajoute des images. La couche présente
/// `multiplicateur` images par image réelle : à la fréquence de l'écran, les présents dépassent ce
/// que le compositeur peut afficher, les images ne reviennent plus au jeu et la génération
/// s'étouffe après quelques milliers d'images. Le jeu tourne donc à `plafond / multiplicateur` et
/// la couche complète jusqu'au plafond affiché.
fn afmf_paced_cap(cap: u32, multiplier: u32) -> Option<u32> {
    (cap > 0 && multiplier >= 2).then(|| (cap / multiplier).max(15))
}

fn effective_frame_generation(
    global: lapinux_core::config::FrameGeneration,
    per_game: Option<lapinux_core::config::FrameGeneration>,
) -> lapinux_core::config::FrameGeneration {
    per_game.unwrap_or(global).supported()
}

/// Neutralise les couches externes dans l'arbre du jeu, même lorsque la
/// génération est désactivée. Les réglages persistés ne sont pas modifiés.
fn frame_generation_environment(
    frame: lapinux_core::config::FrameGeneration,
    wayland_session: bool,
) -> Vec<(String, Option<String>)> {
    use lapinux_core::config::FrameGeneration;
    let afmf = frame.supported() == FrameGeneration::AfmfLinux;
    // DXVK sur une surface X11 ne propose pas MAILBOX : le moteur de présentation garde alors ses
    // images et la génération s'étouffe après quelques milliers d'images. Le pilote Wayland de
    // Proton crée une surface Wayland, où MAILBOX existe et où les images compagnes reviennent
    // au jeu. La variable n'est posée que lorsque la génération est active et que la session est
    // Wayland : sans elle, Proton garde son pilote X11 habituel.
    let proton_wayland = afmf && wayland_session;
    vec![
        ("DISABLE_AFMF".into(), (!afmf).then(|| "1".into())),
        ("AFMF_ENABLE".into(), afmf.then(|| "1".into())),
        (
            "PROTON_ENABLE_WAYLAND".into(),
            proton_wayland.then(|| "1".into()),
        ),
        ("DISABLE_LSFGVK".into(), Some("1".into())),
        ("NVPRESENT_ENABLE_SMOOTH_MOTION".into(), None),
        ("PROTON_USE_OPTISCALER".into(), None),
    ]
}

/// Signale des indices explicites d'une installation manuelle d'OptiScaler.
/// Ne retire ni ne modifie aucun fichier, même en cas d'identification positive.
fn check_external_optiscaler(exe: &Path, root: &Path) -> Result<()> {
    let parent = exe.parent().ok_or_else(|| {
        lapinux_core::Error::Other("dossier de l'exécutable introuvable".into())
    })?;
    for folder in [parent, root] {
        for entry in std::fs::read_dir(folder).map_err(|e| {
            lapinux_core::Error::Other(format!("vérification OptiScaler dans {} : {e}", folder.display()))
        })? {
            let entry = entry.map_err(|e| lapinux_core::Error::Other(e.to_string()))?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.eq_ignore_ascii_case("OptiScaler.dll") || name.eq_ignore_ascii_case("OptiScaler.ini") {
                return Err(lapinux_core::Error::Other(format!(
                    "afmf-linux : fichier pouvant appartenir à OptiScaler détecté : {}. Vérifie son installation avant le lancement ; aucun fichier n'a été supprimé.",
                    entry.path().display()
                )));
            }
        }
    }
    Ok(())
}

/// Variables qui envoient les caches de shaders des jeux dans un dossier
/// court, hors du préfixe Proton : les chemins par défaut y sont longs et
/// certains pilotes refusent les chemins dépassant leur limite.
const SHADER_CACHE_VARIABLES: [&str; 4] = [
    "DXVK_CACHE_PATH",
    "DXVK_STATE_CACHE_PATH",
    "MESA_SHADER_CACHE_DIR",
    "VKD3D_SHADER_CACHE_PATH",
];

/// Prépare `<bibliothèque>/cache/<appid>` et les variables qui y pointent.
/// Une variable déjà posée par l'utilisateur n'est jamais remplacée ; le
/// dossier n'est créé que si au moins une variable doit être ajoutée.
fn shader_cache_environment(
    library_root: &std::path::Path,
    folder: &str,
    user_env: &[(String, String)],
) -> (Option<std::path::PathBuf>, Vec<(String, String)>) {
    let path = library_root.join("Shaders").join(folder);
    let variables: Vec<(String, String)> = SHADER_CACHE_VARIABLES
        .iter()
        .filter(|key| {
            !user_env
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case(key))
        })
        .map(|key| (key.to_string(), path.display().to_string()))
        .collect();
    if variables.is_empty() {
        return (None, variables);
    }
    match std::fs::create_dir_all(&path) {
        Ok(()) => (Some(path), variables),
        Err(error) => {
            tracing::warn!(
                "caches de shaders : création de {} impossible ({error})",
                path.display()
            );
            (None, Vec::new())
        }
    }
}

/// Résout les options sans écrire dans le jeu ni son préfixe. Une variable
/// manuelle reste prioritaire : un conflit est signalé plutôt qu'écrasé.
fn compatibility_env(
    settings: &lapinux_core::model::GameSettings,
    native: bool,
    frame: lapinux_core::config::FrameGeneration,
    injector: bool,
    vkbasalt: bool,
    inherited: impl Fn(&str) -> Option<String>,
) -> Result<Vec<(String, Option<String>)>> {
    use lapinux_core::config::FrameGeneration;
    let c = &settings.compatibility;
    if native {
        return Ok(Vec::new());
    }
    let value = |key: &str| {
        settings
            .env
            .iter()
            .rev()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.clone())
            .or_else(|| inherited(key))
    };
    let active = |key: &str| value(key).is_some_and(|v| !v.is_empty() && v != "0");
    let fail = |message: &str| lapinux_core::Error::Other(message.into());
    if (c.wined3d && c.dxvk_d3d8)
        || (c.wined3d && active("PROTON_DXVK_D3D8"))
        || (c.dxvk_d3d8 && active("PROTON_USE_WINED3D"))
    {
        return Err(fail(
            "WineD3D et DXVK D3D8 ne doivent pas être activés ensemble",
        ));
    }
    if c.dxvk_legacy_line().is_some() && active("PROTON_USE_WINED3D") {
        return Err(fail(
            "les correctifs DXVK D3D9 ne peuvent pas être combinés avec PROTON_USE_WINED3D",
        ));
    }
    let compat_flags = value("STEAM_COMPAT_CONFIG").unwrap_or_default();
    let has_compat_flag = |needle: &str| {
        compat_flags
            .split([',', ';', ' '])
            .any(|s| s.eq_ignore_ascii_case(needle))
    };
    if (c.wined3d && has_compat_flag("dxvkd3d8"))
        || ((c.dxvk_d3d8 || c.dxvk_legacy_line().is_some()) && has_compat_flag("wined3d"))
    {
        return Err(fail(
            "STEAM_COMPAT_CONFIG contient une option graphique incompatible",
        ));
    }
    let dll_overrides = value("WINEDLLOVERRIDES").unwrap_or_default();
    if c.wined3d
        && dll_overrides.split(';').any(|rule| {
            rule.split('=').next().unwrap_or("").split(',').any(|name| {
                let name = name.trim().to_ascii_lowercase();
                let name = name.strip_suffix(".dll").unwrap_or(&name);
                ["d3d8", "d3d9", "d3d10", "d3d11", "dxgi"].contains(&name)
            })
        })
    {
        return Err(fail(
            "WINEDLLOVERRIDES contient une DLL pouvant entrer en conflit avec WineD3D",
        ));
    }
    if c.wined3d
        && (vkbasalt
            || active("ENABLE_VKBASALT")
            || active("AFMF_ENABLE")
            || active("PROTON_USE_OPTISCALER")
            || value("DXVK_CONFIG").is_some()
            || value("DXVK_CONFIG_FILE").is_some()
            || c.dxvk_legacy_line().is_some()
            || !settings.dxvk.is_empty()
            || injector
            || matches!(
                frame,
                FrameGeneration::AfmfLinux | FrameGeneration::OptiScaler
            ))
    {
        return Err(fail("WineD3D est incompatible avec les réglages DXVK, l'injecteur ou la génération d'images sélectionnés"));
    }
    let mut output = Vec::new();
    for (enabled, key) in [
        (c.wined3d, "PROTON_USE_WINED3D"),
        (c.dxvk_d3d8, "PROTON_DXVK_D3D8"),
        (c.old_gl_string, "PROTON_OLD_GL_STRING"),
        (c.no_fsync, "PROTON_NO_FSYNC"),
        (c.no_esync, "PROTON_NO_ESYNC"),
        (c.no_ntsync, "PROTON_NO_NTSYNC"),
    ] {
        if enabled {
            if value(key).is_some() {
                return Err(fail(&format!(
                    "{key} est déjà défini manuellement ou hérité"
                )));
            }
            output.push((key.into(), Some("1".into())));
        }
    }
    // Certaines versions de Proton activent D3D8 par défaut avec WineD3D.
    if c.wined3d {
        if value("PROTON_DXVK_D3D8").is_some() {
            return Err(fail(
                "PROTON_DXVK_D3D8 est déjà défini manuellement ou hérité",
            ));
        }
        output.push(("PROTON_DXVK_D3D8".into(), Some("0".into())));
    }
    if let Some(legacy) = c.dxvk_legacy_line() {
        if value("DXVK_CONFIG").is_some() || value("DXVK_CONFIG_FILE").is_some() {
            return Err(fail(
                "DXVK_CONFIG ou DXVK_CONFIG_FILE est déjà défini manuellement ou hérité",
            ));
        }
        let line = settings
            .dxvk
            .config_line()
            .map(|base| format!("{base}; {legacy}"))
            .unwrap_or(legacy);
        output.push(("DXVK_CONFIG".into(), Some(line)));
    }
    if c.vkd3d_no_dxr {
        let manual = value("VKD3D_CONFIG").unwrap_or_default();
        if manual.split([',', ';']).any(|item| {
            matches!(
                item.trim().to_ascii_lowercase().as_str(),
                "dxr" | "dxr11" | "dxr12" | "nodxr"
            )
        }) {
            return Err(fail("VKD3D_CONFIG contient déjà une option DXR"));
        }
        let merged = if manual.trim().is_empty() {
            "nodxr".into()
        } else {
            format!("{manual},nodxr")
        };
        output.push(("VKD3D_CONFIG".into(), Some(merged)));
    }
    Ok(output)
}

/// Prépare un préréglage local sans toucher aux fichiers du jeu ni remplacer
/// un fichier existant modifié. Les chemins entre guillemets suivent vkBasalt.
fn vkbasalt_preset(
    data_dir: &std::path::Path,
    appid: u32,
    effect: lapinux_core::config::VkBasaltEffect,
) -> Result<std::path::PathBuf> {
    use lapinux_core::config::VkBasaltEffect;
    use std::io::Write;
    fn write_owned(path: &std::path::Path, data: &[u8]) -> Result<()> {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(mut file) => file
                .write_all(data)
                .map_err(|e| lapinux_core::Error::Other(e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if std::fs::read(path).ok().as_deref() == Some(data) {
                    Ok(())
                } else {
                    Err(lapinux_core::Error::Other(format!(
                        "vkBasalt: fichier existant différent : {}",
                        path.display()
                    )))
                }
            }
            Err(e) => Err(lapinux_core::Error::Other(e.to_string())),
        }
    }
    let base = data_dir.join("graphics/vkbasalt-presets-v1");
    std::fs::create_dir_all(&base).map_err(|e| lapinux_core::Error::Other(e.to_string()))?;
    let (name, contents) = match effect {
        VkBasaltEffect::Existing => {
            return Err(lapinux_core::Error::Other("no preset selected".into()))
        }
        VkBasaltEffect::Cas => ("cas", "effects = cas\n".to_string()),
        VkBasaltEffect::Fxaa => ("fxaa", "effects = fxaa\n".to_string()),
        VkBasaltEffect::Smaa => ("smaa", "effects = smaa\n".to_string()),
        VkBasaltEffect::Ascii => {
            let dir = base.join("ascii");
            std::fs::create_dir_all(&dir).map_err(|e| lapinux_core::Error::Other(e.to_string()))?;
            let dir = std::fs::canonicalize(dir)
                .map_err(|e| lapinux_core::Error::Other(e.to_string()))?;
            let text = dir.to_str().ok_or_else(|| {
                lapinux_core::Error::Other("vkBasalt shader path is not UTF-8".into())
            })?;
            if text.contains(['"', '\n', '\r']) {
                return Err(lapinux_core::Error::Other(
                    "vkBasalt shader path contains unsupported characters".into(),
                ));
            }
            for (name, content) in [
                (
                    "ASCII.fx",
                    include_str!("../../../assets/graphics/shaders/ASCII.fx"),
                ),
                (
                    "ReShade.fxh",
                    include_str!("../../../assets/graphics/shaders/ReShade.fxh"),
                ),
                (
                    "ReShadeUI.fxh",
                    include_str!("../../../assets/graphics/shaders/ReShadeUI.fxh"),
                ),
                (
                    "LICENSE.SweetFX",
                    include_str!("../../../assets/graphics/shaders/LICENSE.SweetFX"),
                ),
            ] {
                write_owned(&dir.join(name), content.as_bytes())?;
            }
            ("ascii", format!("effects = ascii\nascii = \"{text}/ASCII.fx\"\nreshadeIncludePath = \"{text}\"\ndepthCapture = off\n"))
        }
    };
    let path = base.join(format!("{appid}-{name}.conf"));
    write_owned(&path, contents.as_bytes())?;
    Ok(path)
}

/// Ajoute un dossier de layers sans écraser AFMF ni une variable fournie par
/// le joueur. Le chemin Lapinux passe en premier pour qu'il gagne face à un
/// layer système du même nom.
fn add_implicit_layer_path(env: &mut Vec<(String, Option<String>)>, dir: &std::path::Path) {
    let dir = dir.to_string_lossy();
    if let Some((_, value)) = env
        .iter_mut()
        .find(|(key, _)| key.eq_ignore_ascii_case("VK_ADD_IMPLICIT_LAYER_PATH"))
    {
        match value {
            Some(existing) if !existing.is_empty() => {
                if !existing.split(':').any(|path| path == dir) {
                    *existing = format!("{dir}:{existing}");
                }
            }
            _ => *value = Some(dir.into_owned()),
        }
        return;
    }
    let mut value = dir.into_owned();
    if let Some(existing) = std::env::var("VK_ADD_IMPLICIT_LAYER_PATH")
        .ok()
        .filter(|value| !value.is_empty())
    {
        value.push(':');
        value.push_str(&existing);
    }
    env.push(("VK_ADD_IMPLICIT_LAYER_PATH".into(), Some(value)));
}

/// Écrit le multiplicateur dans le fichier de contrôle d'une session.
///
/// La couche lit ce fichier à chaque image : le remplacer par un `rename`
/// donne au jeu le nouveau multiplicateur sans redémarrer quoi que ce soit.
/// Le fichier temporaire naît en 0600 pour que la couche, qui refuse tout
/// autre propriétaire ou des droits partagés, puisse le lire.
pub(crate) fn write_afmf_multiplier(
    path: &std::path::Path,
    multiplier: u32,
) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let multiplier = lapinux_core::config::afmf_multiplier_or_default(multiplier);
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "afmf.control".into());
    let temp = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&temp);
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(multiplier.to_string().as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod graphics_tests {
    use super::*;
    use lapinux_core::config::VkBasaltEffect;

    #[test]
    fn native_bridge_selection_respects_preferences_and_architecture() {
        let dir = tempfile::tempdir().unwrap();
        let shim = dir.path().join("libsteam_api.so");
        let exe = dir.path().join("game");
        let header = |class: u8, machine: u16| {
            let mut bytes = [0u8; 20];
            bytes[..4].copy_from_slice(b"\x7fELF");
            bytes[4] = class;
            bytes[5] = 1;
            bytes[18..20].copy_from_slice(&machine.to_le_bytes());
            bytes
        };
        // Petit ELF synthétique : en-tête, segment LOAD, table dynamique et SONAME.
        let mut library = vec![0u8; 256];
        library[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        library[16..18].copy_from_slice(&3u16.to_le_bytes());
        library[18..20].copy_from_slice(&62u16.to_le_bytes());
        library[20..24].copy_from_slice(&1u32.to_le_bytes());
        library[32..40].copy_from_slice(&64u64.to_le_bytes());
        library[52..54].copy_from_slice(&64u16.to_le_bytes());
        library[54..56].copy_from_slice(&56u16.to_le_bytes());
        library[56..58].copy_from_slice(&2u16.to_le_bytes());
        library[64..68].copy_from_slice(&1u32.to_le_bytes());
        library[96..104].copy_from_slice(&256u64.to_le_bytes());
        library[120..124].copy_from_slice(&2u32.to_le_bytes());
        library[128..136].copy_from_slice(&176u64.to_le_bytes());
        library[152..160].copy_from_slice(&64u64.to_le_bytes());
        for (i, (tag, value)) in [(5u64, 240u64), (10, 16), (14, 0), (0, 0)]
            .into_iter()
            .enumerate()
        {
            library[176 + i * 16..184 + i * 16].copy_from_slice(&tag.to_le_bytes());
            library[184 + i * 16..192 + i * 16].copy_from_slice(&value.to_le_bytes());
        }
        library[240..256].copy_from_slice(b"libsteam_api.so\0");
        std::fs::write(&shim, &library).unwrap();
        std::fs::write(&exe, header(2, 62)).unwrap();
        let resolve = |pref, explicit| native_shim_path(pref, explicit, dir.path(), None, &exe);
        assert_eq!(resolve(None, None).unwrap(), None);
        assert_eq!(resolve(Some(false), Some(shim.clone())).unwrap(), None);
        assert_eq!(
            resolve(Some(true), None).unwrap(),
            Some(shim.canonicalize().unwrap())
        );
        assert_eq!(resolve(None, Some(shim.clone())).unwrap(), None);
        assert_eq!(
            resolve(Some(true), Some(shim.clone())).unwrap(),
            Some(shim.canonicalize().unwrap())
        );
        std::fs::write(&exe, header(1, 3)).unwrap();
        assert!(resolve(Some(true), None).is_err());
        std::fs::write(&exe, header(2, 62)).unwrap();
        std::fs::write(&shim, &library[..20]).unwrap();
        assert!(resolve(Some(true), None).is_err());
        library[240] = b'X';
        std::fs::write(&shim, &library).unwrap();
        assert!(resolve(Some(true), None).is_err());
        std::fs::write(&shim, header(1, 3)).unwrap();
        assert!(resolve(Some(true), None).is_err());
    }

    #[test]
    fn legacy_frame_generation_engines_are_inactive() {
        use lapinux_core::config::FrameGeneration::*;
        for legacy in [OptiScaler, NvidiaSmoothMotion, LsfgVk] {
            assert_eq!(effective_frame_generation(legacy, None), Off);
            assert_eq!(effective_frame_generation(AfmfLinux, Some(legacy)), Off);
        }
        assert_eq!(effective_frame_generation(AfmfLinux, None), AfmfLinux);
        assert_eq!(effective_frame_generation(AfmfLinux, Some(Off)), Off);
    }

    #[test]
    fn frame_generation_environment_disables_competing_layers_even_when_off() {
        use lapinux_core::config::FrameGeneration::*;
        for mode in [Off, NvidiaSmoothMotion, OptiScaler, LsfgVk, AfmfLinux] {
            for wayland in [false, true] {
                let env = frame_generation_environment(mode, wayland);
                let value =
                    |name: &str| env.iter().find(|(key, _)| key == name).unwrap().1.as_deref();
                assert_eq!(value("DISABLE_LSFGVK"), Some("1"));
                assert_eq!(value("NVPRESENT_ENABLE_SMOOTH_MOTION"), None);
                assert_eq!(value("PROTON_USE_OPTISCALER"), None);
                assert_eq!(value("DISABLE_AFMF"), (mode != AfmfLinux).then_some("1"));
                assert_eq!(value("AFMF_ENABLE"), (mode == AfmfLinux).then_some("1"));
                // Le pilote Wayland de Proton n'est posé que pour la couche afmf-linux et
                // seulement dans une session Wayland.
                assert_eq!(
                    value("PROTON_ENABLE_WAYLAND"),
                    (mode == AfmfLinux && wayland).then_some("1")
                );
            }
        }
    }

    #[test]
    fn frame_generation_paces_the_game_below_the_display_rate() {
        // Le plafond affiché est divisé par le multiplicateur : la couche complète ensuite.
        assert_eq!(afmf_paced_cap(165, 2), Some(82));
        assert_eq!(afmf_paced_cap(165, 3), Some(55));
        assert_eq!(afmf_paced_cap(165, 4), Some(41));
        // Sans plafond ou sans génération, rien n'est imposé au jeu.
        assert_eq!(afmf_paced_cap(0, 2), None);
        assert_eq!(afmf_paced_cap(165, 1), None);
        // Un plancher évite une cadence inutilisable sur un petit plafond.
        assert_eq!(afmf_paced_cap(20, 2), Some(15));
    }

    #[test]
    fn external_optiscaler_markers_block_without_modifying_files() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let exe = bin.join("game.exe");
        std::fs::write(&exe, b"game").unwrap();
        assert!(check_external_optiscaler(&exe, dir.path()).is_ok());
        for (folder, name) in [(&bin, "OPTISCALER.INI"), (&dir.path().to_path_buf(), "optiscaler.dll")] {
            let marker = folder.join(name);
            std::fs::write(&marker, b"user file").unwrap();
            assert!(check_external_optiscaler(&exe, dir.path()).is_err());
            assert_eq!(std::fs::read(&marker).unwrap(), b"user file");
            std::fs::remove_file(marker).unwrap();
        }
    }

    #[test]
    fn compatibility_options_are_opt_in_reversible_and_preserve_legacy_settings() {
        use lapinux_core::config::FrameGeneration;
        use lapinux_core::model::GameSettings;
        let mut s: GameSettings = serde_json::from_str(r#"{"env":[["OTHER","kept"]]}"#).unwrap();
        let none = |_key: &str| None;
        assert!(
            compatibility_env(&s, false, FrameGeneration::Off, false, false, none)
                .unwrap()
                .is_empty()
        );
        s.compatibility.old_gl_string = true;
        s.compatibility.no_fsync = true;
        s.compatibility.no_esync = true;
        s.compatibility.no_ntsync = true;
        let env = compatibility_env(&s, false, FrameGeneration::Off, false, false, none).unwrap();
        for key in [
            "PROTON_OLD_GL_STRING",
            "PROTON_NO_FSYNC",
            "PROTON_NO_ESYNC",
            "PROTON_NO_NTSYNC",
        ] {
            assert!(env.contains(&(key.into(), Some("1".into()))));
        }
        assert!(
            compatibility_env(&s, true, FrameGeneration::Off, false, false, none)
                .unwrap()
                .is_empty()
        );
        s.compatibility = Default::default();
        assert!(
            compatibility_env(&s, false, FrameGeneration::Off, false, false, none)
                .unwrap()
                .is_empty()
        );
        assert_eq!(s.env, vec![("OTHER".into(), "kept".into())]);
    }

    #[test]
    fn compatibility_options_check_wined3d_and_injector_conflicts() {
        use lapinux_core::config::FrameGeneration::*;
        use lapinux_core::model::GameSettings;
        let none = |_key: &str| None;
        let mut s = GameSettings::default();
        s.compatibility.wined3d = true;
        let env = compatibility_env(&s, false, Off, false, false, none).unwrap();
        assert!(env.contains(&("PROTON_DXVK_D3D8".into(), Some("0".into()))));
        assert!(compatibility_env(&s, false, AfmfLinux, false, false, none).is_err());
        assert!(compatibility_env(&s, false, OptiScaler, false, false, none).is_err());
        assert!(compatibility_env(&s, false, Off, true, false, none).is_err());
        assert!(compatibility_env(&s, false, Off, false, true, none).is_err());
        s.env
            .push(("WINEDLLOVERRIDES".into(), "DXGI.DLL=n,b".into()));
        assert!(compatibility_env(&s, false, Off, false, false, none).is_err());
        s.env.clear();
        s.env
            .push(("STEAM_COMPAT_CONFIG".into(), "foo,dxvkd3d8".into()));
        assert!(compatibility_env(&s, false, Off, false, false, none).is_err());
        s.env.clear();
        s.compatibility.dxvk_d3d8 = true;
        assert!(compatibility_env(&s, false, Off, false, false, none).is_err());
        s.compatibility.wined3d = false;
        s.env.push(("PROTON_USE_WINED3D".into(), "1".into()));
        assert!(compatibility_env(&s, false, Off, false, false, none).is_err());
        s.env.clear();
        s.compatibility.dxvk_d3d8 = false;
        s.compatibility.wined3d = true;
        assert!(compatibility_env(&s, false, Off, false, false, |key| (key
            == "PROTON_DXVK_D3D8")
            .then(|| "1".into()))
        .is_err());
    }

    #[test]
    fn compatibility_options_compose_dxvk_and_vkd3d_without_overwriting() {
        use lapinux_core::config::FrameGeneration::Off;
        use lapinux_core::model::GameSettings;
        let mut s = GameSettings::default();
        s.dxvk.device_mb = Some(1024);
        s.compatibility.d3d9_mode_count = true;
        s.compatibility.d3d9_cube_depth = true;
        s.compatibility.d3d9_sampler_type = true;
        s.compatibility.vkd3d_no_dxr = true;
        s.env.push(("VKD3D_CONFIG".into(), "foo;bar".into()));
        let none = |_key: &str| None;
        let env = compatibility_env(&s, false, Off, false, false, none).unwrap();
        let dxvk = env
            .iter()
            .find(|(k, _)| k == "DXVK_CONFIG")
            .unwrap()
            .1
            .as_deref()
            .unwrap();
        for fragment in [
            "dxgi.maxDeviceMemory = 1024",
            "d3d9.modeCountCompatibility = True",
            "d3d9.supportCubeDepthFormats = True",
            "d3d9.forceSamplerTypeSpecConstants = True",
        ] {
            assert!(dxvk.contains(fragment));
        }
        assert!(env.contains(&("VKD3D_CONFIG".into(), Some("foo;bar,nodxr".into()))));
        s.env.push(("DXVK_CONFIG_FILE".into(), "mine".into()));
        assert!(compatibility_env(&s, false, Off, false, false, none).is_err());
        s.env.pop();
        s.env[0].1 = "dxr,foo".into();
        assert!(compatibility_env(&s, false, Off, false, false, none).is_err());
        s.env.clear();
        assert!(
            compatibility_env(&s, false, Off, false, false, |key| (key == "DXVK_CONFIG")
                .then(|| "user".into()))
            .is_err()
        );
        assert!(compatibility_env(&s, false, Off, false, false, |key| (key
            == "PROTON_USE_WINED3D")
            .then(|| "1".into()))
        .is_err());
    }

    #[test]
    fn ascii_preset_is_local_idempotent_and_preserves_modified_files() {
        let root = tempfile::tempdir().unwrap();
        let path = vkbasalt_preset(root.path(), 42, VkBasaltEffect::Ascii).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("effects = ascii"));
        let shaders = root.path().join("graphics/vkbasalt-presets-v1/ascii");
        assert!(shaders.join("ASCII.fx").is_file());
        assert!(shaders.join("LICENSE.SweetFX").is_file());
        assert_eq!(
            vkbasalt_preset(root.path(), 42, VkBasaltEffect::Ascii).unwrap(),
            path
        );
        std::fs::write(shaders.join("ASCII.fx"), "modified").unwrap();
        assert!(vkbasalt_preset(root.path(), 42, VkBasaltEffect::Ascii).is_err());
        assert_eq!(
            std::fs::read_to_string(shaders.join("ASCII.fx")).unwrap(),
            "modified"
        );
    }

    #[test]
    fn le_runtime_vkbasalt_rejoint_le_layer_afmf_sans_ecraser() {
        let mut env = vec![(
            "VK_ADD_IMPLICIT_LAYER_PATH".into(),
            Some("/runtime/afmf".into()),
        )];
        add_implicit_layer_path(&mut env, std::path::Path::new("/runtime/vkbasalt"));
        assert_eq!(
            env.iter()
                .find(|(key, _)| key == "VK_ADD_IMPLICIT_LAYER_PATH")
                .unwrap()
                .1
                .as_deref(),
            Some("/runtime/vkbasalt:/runtime/afmf")
        );
        // Un second passage ne duplique pas le même dossier.
        add_implicit_layer_path(&mut env, std::path::Path::new("/runtime/vkbasalt"));
        assert_eq!(
            env.iter()
                .find(|(key, _)| key == "VK_ADD_IMPLICIT_LAYER_PATH")
                .unwrap()
                .1
                .as_deref(),
            Some("/runtime/vkbasalt:/runtime/afmf")
        );
    }

    #[test]
    fn l_architecture_du_jeu_choisit_le_bon_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("game");
        let mut elf = [0u8; 20];
        elf[..4].copy_from_slice(b"\x7fELF");
        elf[4] = 1;
        std::fs::write(&exe, elf).unwrap();
        assert_eq!(game_bits(&exe), Some(32));
        elf[4] = 2;
        std::fs::write(&exe, elf).unwrap();
        assert_eq!(game_bits(&exe), Some(64));

        let mut pe = vec![0u8; 70];
        pe[..2].copy_from_slice(b"MZ");
        pe[0x3c..0x40].copy_from_slice(&64u32.to_le_bytes());
        pe[64..68].copy_from_slice(b"PE\0\0");
        pe[68..70].copy_from_slice(&0x014cu16.to_le_bytes());
        std::fs::write(&exe, &pe).unwrap();
        assert_eq!(game_bits(&exe), Some(32));
        pe[68..70].copy_from_slice(&0x8664u16.to_le_bytes());
        std::fs::write(&exe, &pe).unwrap();
        assert_eq!(game_bits(&exe), Some(64));

        std::fs::write(&exe, b"#!/bin/sh\n").unwrap();
        assert_eq!(game_bits(&exe), None);
    }
}

impl Shared {
    /// Le pont Steamworks ne concerne que les jeux Steam Windows installés.
    fn steam_bridge_game(&self, appid: u32) -> Result<Game> {
        let game = self
            .store
            .game(appid)
            .map_err(|e| lapinux_core::Error::Other(e.to_string()))?
            .ok_or_else(|| lapinux_core::Error::Other("jeu introuvable".into()))?;
        if !game.installed || game.native || game.store != lapinux_core::stores::Store::Steam {
            return Err(lapinux_core::Error::Other(
                "pont Steamworks réservé aux jeux Steam Windows installés".into(),
            ));
        }
        Ok(game)
    }

    /// DLL laissés par une session Steamworks interrompue, chemins relatifs au
    /// dossier du jeu pour que la fiche reste lisible.
    pub fn steam_bridge_status(&self, appid: u32) -> Result<Envelope> {
        let Ok(game) = self.steam_bridge_game(appid) else {
            return Ok(Envelope::ok(&Vec::<String>::new()));
        };
        let root = game.install_dir();
        let paths: Vec<String> = crate::steam_shim::stale_shim_targets(&root, &self.paths.data_dir)
            .into_iter()
            .map(|path| {
                path.strip_prefix(&root)
                    .unwrap_or(&path)
                    .display()
                    .to_string()
            })
            .collect();
        Ok(Envelope::ok(&paths))
    }

    /// Restaure les DLL d'origine. Le jeu doit être arrêté : remplacer un DLL
    /// encore chargé ferait échouer la réparation au mieux, casserait la
    /// session Steamworks au pire.
    pub fn repair_steam_bridge(&self, appid: u32) -> Result<Envelope> {
        let _guard = self.save_operation.try_lock().map_err(|_| {
            lapinux_core::Error::Other(
                "une opération de sauvegarde ou de lancement est déjà en cours".into(),
            )
        })?;
        let game = self.steam_bridge_game(appid)?;
        if self
            .session_info()
            .is_some_and(|session| session.appid == appid)
        {
            return Err(lapinux_core::Error::Other(
                "ferme le jeu lancé par Lapinux avant de réparer le pont".into(),
            ));
        }
        let root = game.install_dir();
        let restored = crate::steam_shim::repair_stale_shims(&root, &self.paths.data_dir)
            .map_err(lapinux_core::Error::Other)?;
        if restored.is_empty() {
            return Err(lapinux_core::Error::Other(
                "aucun pont Steamworks orphelin à réparer".into(),
            ));
        }
        let paths: Vec<String> = restored
            .iter()
            .map(|path| {
                path.strip_prefix(&root)
                    .unwrap_or(path)
                    .display()
                    .to_string()
            })
            .collect();
        Ok(Envelope::ok(&paths))
    }

    /// Prépare et lance directement un jeu installé, sans client tiers.
    pub fn launch(self: &Arc<Self>, appid: u32, dry_run: bool) -> Result<Envelope> {
        let _guard = self.save_operation.try_lock().map_err(|_| {
            lapinux_core::Error::Other(
                "une opération de sauvegarde ou de lancement est déjà en cours".into(),
            )
        })?;
        // Relit le fichier : un réglage modifié à la main (Proton, raccourci)
        // doit s'appliquer à cette session, pas rester celui du démarrage.
        self.reload_config();
        let mut game = self
            .store
            .game(appid)
            .map_err(|e| lapinux_core::Error::Other(e.to_string()))?
            .ok_or_else(|| lapinux_core::Error::Other("jeu introuvable".into()))?;
        if !game.installed {
            return Err(lapinux_core::Error::Other(
                "le jeu n'est pas installé dans une bibliothèque connue".into(),
            ));
        }
        // Les premières installations de jeux itch.io en archive Windows
        // n'avaient pas trouvé leur `.exe` : le fichier n'a pas de bit
        // exécutable Unix. On répare l'entrée existante au premier lancement,
        // sans imposer un nouveau téléchargement.
        if crate::stores::refresh_game_executable(&mut game) {
            tracing::info!(
                "exécutable retrouvé pour {appid} : {} (native={})",
                game.exe
                    .as_ref()
                    .map(|exe| exe.display().to_string())
                    .unwrap_or_default(),
                game.native
            );
            if let Err(error) = self.store.save_game(&game) {
                tracing::warn!(
                    "jeu {appid} non enregistré après réparation de l'exécutable : {error}"
                );
            }
        }
        if self.session.lock().map(|s| s.is_some()).unwrap_or(false) {
            return Err(lapinux_core::Error::Other(
                "une session est déjà en cours".into(),
            ));
        }
        // Un anti-triche protège son processus : pont Steamworks, injecteurs
        // et enregistrement automatique restent coupés tant que le joueur ne
        // les a pas réclamés explicitement pour ce jeu.
        let (settings, protected) = self.protected_settings(&game);
        game.settings = settings;
        if protected {
            tracing::info!(
                "jeu {appid} protégé par un anti-triche : pont, injecteurs et enregistrement automatique coupés par défaut"
            );
            // Un remplacement de DLL DLSS laissé en place ferait démarrer le
            // jeu avec des bibliothèques que l'anti-triche peut refuser.
            // Un simple `--dry-run` ne touche à rien : il décrit le lancement.
            if !dry_run && self.dlss_restore_if_blocked(&game, protected) {
                tracing::info!(
                    "jeu {appid} protégé : bibliothèques DLSS d'origine remises avant le lancement"
                );
            }
        }
        let config = self.config_snapshot();
        let protons = self.protons_cached();
        let wanted = game
            .settings
            .proton
            .clone()
            .unwrap_or_else(|| config.launch.proton.clone());
        let proton = if game.native {
            None
        } else if wanted == "auto" {
            lapinux_proton::recommended(&protons).cloned()
        } else {
            protons.iter().find(|p| p.name == wanted).cloned()
        };
        if !game.native && proton.is_none() {
            return Err(lapinux_core::Error::Other(
                "aucun Proton utilisable : installez GE-Proton depuis l'écran Protons".into(),
            ));
        }
        let exe = game
            .exe
            .clone()
            .ok_or_else(|| lapinux_core::Error::Other("exécutable du jeu introuvable".into()))?;
        let frame_generation = effective_frame_generation(
            config.graphics.frame_generation,
            game.settings.frame_generation,
        );
        let injector_overrides = self.launch_injector_override(&game, &exe, frame_generation)?;
        if frame_generation == lapinux_core::config::FrameGeneration::AfmfLinux && !game.native {
            check_external_optiscaler(&exe, &game.install_dir())?;
        }
        let compatibility = compatibility_env(
            &game.settings,
            game.native,
            frame_generation,
            injector_overrides.is_some(),
            game.settings.vkbasalt.unwrap_or(config.graphics.vkbasalt),
            |key| std::env::var(key).ok(),
        )?;
        let prefix = lapinux_proton::prefix_for(&game, &self.paths.data_dir);
        lapinux_proton::ensure_prefix(&prefix, appid)
            .map_err(|e| lapinux_core::Error::Other(e.to_string()))?;
        // La limite que le jeu s'impose lui-même se lève avant le démarrage :
        // le jeu ne relit ses fichiers de réglages qu'à ce moment-là.
        self.sync_unlock_fps(appid);

        // umu-run exige un Proton complet (toolmanifest.vdf) ; un Proton
        // partiel est lancé directement par son script, sans umu.
        let umu_ready = proton
            .as_ref()
            .map(|p| p.path.join("toolmanifest.vdf").is_file())
            .unwrap_or(false);
        // Un jeu Steam suit le chemin de Steam : Proton directement, avec le
        // `compatdata` de sa bibliothèque. umu-run reste aux jeux de boutique.
        // Un jeu Steam — même installé par Lapinux — garde le chemin Steam :
        // umu-run ne transmet ni SteamAppId ni le client Steam, et Steamworks
        // ne peut pas s'initialiser. Les jeux de boutique restent sur umu.
        let umu = game.origin != lapinux_core::model::GameOrigin::Steam
            && game.store != lapinux_core::stores::Store::Steam
            && game.prefix.is_none()
            && umu_ready
            && lapinux_proton::umu_run_path().is_some();
        let steam_game = game.store == lapinux_core::stores::Store::Steam;
        // Langue du jeu : celle choisie pour lui, sinon celle des réglages
        // généraux. Wine et les jeux lisent la locale de l'environnement.
        let mut extra_env = game.settings.env.clone();
        // Les caches de shaders DXVK, Mesa et VKD3D vivent dans un dossier
        // court de la bibliothèque, créé au lancement.
        let (shader_cache, shader_cache_vars) = shader_cache_environment(
            &config.library_root(),
            &crate::saves::folder_label(&game),
            &game.settings.env,
        );
        if let Some(path) = &shader_cache {
            tracing::info!("caches de shaders du jeu {appid} : {}", path.display());
        }
        extra_env.extend(shader_cache_vars.iter().cloned());
        // Les réglages DXVK du jeu passent par `DXVK_CONFIG` : une ligne
        // d'options, lue par DXVK au chargement de ses bibliothèques. Une
        // variable posée à la main par l'utilisateur l'emporte : elle est
        // dans ses réglages, pas dans les nôtres.
        if game.settings.compatibility.dxvk_legacy_line().is_none() {
            if let Some(config) = game.settings.dxvk.config_line() {
                let manual = extra_env.iter().any(|(key, _)| {
                    key.eq_ignore_ascii_case("DXVK_CONFIG")
                        || key.eq_ignore_ascii_case("DXVK_CONFIG_FILE")
                });
                if manual {
                    tracing::info!(
                    "réglages DXVK du jeu {appid} ignorés : DXVK_CONFIG vient des variables du jeu"
                );
                } else {
                    tracing::info!("réglages DXVK du jeu {appid} : {config}");
                    extra_env.push(("DXVK_CONFIG".to_string(), config));
                }
            }
        }
        // Les valeurs composées sont passées au seul arbre du jeu.
        extra_env.retain(|(key, _)| {
            !compatibility
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case(key))
        });
        if let Some(language) = lapinux_core::lang::resolve(
            game.settings.language.as_deref().unwrap_or_default(),
            &config.general.game_language,
        ) {
            extra_env.push(("LANG".to_string(), language.locale.to_string()));
            extra_env.push(("LANGUAGE".to_string(), language.locale.to_string()));
            // Le jeu demande sa langue à Steamworks, pas à `LANG` : le pont
            // répond `GetCurrentGameLanguage` avec ce nom Steam.
            extra_env.push((
                "LAPINUX_STEAM_LANGUAGE".to_string(),
                language.code.to_string(),
            ));
        }
        // Un jeu Linux natif se lance directement, hors du client Steam :
        // rien ne lui transmet son appid, et `SteamAPI_Init` échouerait même
        // client démarré. Les variables que Steam pose lui-même pour un jeu
        // natif font le même office. L'appid d'un jeu de boutique, lui, reste
        // interne à Lapinux : il n'est jamais transmis à Steamworks.
        if game.native && game.store == lapinux_core::stores::Store::Steam {
            // Les variables Unix sont sensibles à la casse et chacune peut
            // être personnalisée indépendamment dans les réglages du jeu.
            for key in ["SteamAppId", "SteamGameId"] {
                if !extra_env.iter().any(|(name, _)| name == key) {
                    extra_env.push((key.to_string(), appid.to_string()));
                }
            }
        }
        // Le pont est limité à l'arbre du jeu, après GameScope et la scène.
        let mut native_preload = None;
        let mut native_parent_preload = None;
        if game.native && game.store == lapinux_core::stores::Store::Steam {
            let current_exe = std::env::current_exe().ok();
            let shim = native_shim_path(
                game.settings.steam_shim,
                std::env::var_os("LAPINUX_STEAMSHIM").map(std::path::PathBuf::from),
                &self.paths.data_dir,
                current_exe.as_deref().and_then(std::path::Path::parent),
                &exe,
            )
            .map_err(lapinux_core::Error::Other)?;
            if let Some(shim) = shim {
                // Le pont garde la priorité Steamworks ; les autres bibliothèques
                // préchargées restent présentes après lui.
                let previous = extra_env
                    .iter()
                    .rev()
                    .find(|(key, _)| key == "LD_PRELOAD")
                    .map(|(_, value)| value.clone())
                    .or_else(|| std::env::var("LD_PRELOAD").ok());
                native_parent_preload = previous.clone();
                let mut list = shim.to_string_lossy().to_string();
                if let Some(previous) = previous.filter(|value| !value.is_empty()) {
                    list.push(':');
                    list.push_str(&previous);
                }
                extra_env.retain(|(key, _)| key != "LD_PRELOAD");
                native_preload = Some(list);
                tracing::info!(
                    "succès Steamworks : pont {} préchargé pour {appid}",
                    shim.display()
                );
            }
        }
        // Pont Windows : le DLL depose ses lots dans un dossier partage, que
        // le daemon lit. Wine y accede par `Z:` (racine du systeme hote).
        if !game.native && game.store == lapinux_core::stores::Store::Steam {
            // Nom du compte transmis au pont : les jeux l'affichent à la place
            // du « Joueur » de repli. La session sociale donne le nom affiché
            // quand elle vit ; sinon la session enregistrée fournit le nom de
            // connexion, déjà connu du daemon.
            let social = self
                .social
                .lock()
                .ok()
                .and_then(|slot| slot.as_ref().map(|social| social.snapshot()));
            let saved = lapinux_steam::session::load_from(&lapinux_steam::session::path_in(
                &self.paths.data_dir,
            ));
            let persona = social
                .as_ref()
                .map(|social| social.persona_name.trim().to_string())
                .filter(|name| !name.is_empty())
                .or_else(|| {
                    saved
                        .as_ref()
                        .map(|session| session.account_name.trim().to_string())
                        .filter(|name| !name.is_empty())
                });
            if let Some(persona) = persona {
                extra_env.retain(|(key, _)| !key.eq_ignore_ascii_case("LAPINUX_STEAM_PERSONA"));
                extra_env.push(("LAPINUX_STEAM_PERSONA".to_string(), persona));
            }
            // Le vrai SteamID évite qu'un jeu demande le profil du compte
            // factice du pont : lobbies, invitations et noms suivent la session.
            let steam_id = social
                .as_ref()
                .map(|social| social.steam_id)
                .filter(|id| *id != 0)
                .or_else(|| {
                    saved
                        .as_ref()
                        .map(|session| session.steam_id)
                        .filter(|id| *id != 0)
                });
            if let Some(steam_id) = steam_id {
                extra_env.retain(|(key, _)| !key.eq_ignore_ascii_case("LAPINUX_STEAM_ID"));
                extra_env.push(("LAPINUX_STEAM_ID".to_string(), steam_id.to_string()));
            }
            let spool = self.paths.data_dir.join("achievements");
            if std::fs::create_dir_all(&spool).is_ok() {
                let windows = format!("Z:{}", spool.to_string_lossy().replace('/', "\\"));
                extra_env.retain(|(key, _)| !key.eq_ignore_ascii_case("LAPINUX_ACHIEVEMENT_SPOOL"));
                extra_env.push(("LAPINUX_ACHIEVEMENT_SPOOL".to_string(), windows));
            }
            // Le pont Windows lit l'atelier dans un manifeste préparé ici :
            // le jeu ne peut pas parcourir directement `steamapps/workshop`
            // depuis son préfixe Proton.
            if let Some(manifest) = crate::steam_shim::write_workshop_manifest(
                &self.paths.data_dir,
                &self.steam_libraries(),
                appid,
            ) {
                let windows = format!("Z:{}", manifest.to_string_lossy().replace('/', "\\"));
                extra_env.retain(|(key, _)| !key.eq_ignore_ascii_case("LAPINUX_WORKSHOP_MANIFEST"));
                extra_env.push(("LAPINUX_WORKSHOP_MANIFEST".to_string(), windows));
            }
        }
        // Invitation acceptée : le jeu démarre en rejoignant le lobby de
        // l'hôte. L'attente est consommée ici, une seule fois, et seulement
        // pour le jeu visé : un lancement normal n'en reçoit rien.
        let mut extra_args = game.settings.args.clone();
        if let Some(join) = crate::social::take_pending_join(appid) {
            let (env, args) = crate::social::join_launch_additions(&join);
            tracing::info!(
                "Steam : lancement de {appid} pour rejoindre le lobby {} à {}",
                join.lobby,
                join.connect
            );
            extra_env.extend(env);
            extra_args.extend(args);
        }
        if injector_overrides.is_some() {
            extra_env.retain(|(key, _)| !key.eq_ignore_ascii_case("WINEDLLOVERRIDES"));
        }
        let plan = lapinux_proton::build(&LaunchOptions {
            appid,
            exe: exe.clone(),
            native: game.native,
            proton: proton.clone(),
            prefix: prefix.clone(),
            steam_root: lapinux_core::paths::steam_roots().first().cloned(),
            umu_run: lapinux_proton::umu_run_path(),
            use_umu: umu,
            extra_env,
            extra_args,
        })
        .map_err(|e| lapinux_core::Error::Other(e.to_string()))?;

        let caps = PerfCaps::detect();
        let scene_visual =
            if !game.settings.performance && config.launch.scene && config.launch.gamescope {
                scene_backdrop(&self.paths, appid)
            } else {
                None
            };
        let mut perf_config = perf_config(
            &config,
            &game,
            scene_cover(&self.paths, appid),
            scene_visual,
        );
        use lapinux_core::config::FrameGeneration;
        // Le pilote Wayland de Proton sert une surface directe au jeu ; dans GameScope, c'est
        // GameScope qui tient la session Wayland et le pilote casse le Xwayland imbriqué.
        let gamescope_active = perf_config.gamescope.enabled && caps.gamescope.is_some();
        let mut frame_env = frame_generation_environment(
            frame_generation,
            std::env::var_os("WAYLAND_DISPLAY").is_some() && !gamescope_active,
        );
        if let Some(overrides) = injector_overrides {
            frame_env.push(("WINEDLLOVERRIDES".into(), Some(overrides)));
        }
        // Renseignés plus bas quand la couche afmf-linux est chargée : le
        // multiplicateur de la session et le fichier qui le change à chaud.
        let mut afmf_multiplier: Option<u32> = None;
        let mut afmf_control: Option<std::path::PathBuf> = None;
        match frame_generation {
            // Les anciens choix restent lisibles mais ne lancent plus de moteur.
            FrameGeneration::Off | FrameGeneration::LsfgVk | FrameGeneration::NvidiaSmoothMotion | FrameGeneration::OptiScaler => {}
            FrameGeneration::AfmfLinux => {
                let optiscaler = game
                    .settings
                    .env
                    .iter()
                    .rev()
                    .find(|(key, _)| key.eq_ignore_ascii_case("PROTON_USE_OPTISCALER"))
                    .map(|(_, value)| value.clone())
                    .or_else(|| std::env::var("PROTON_USE_OPTISCALER").ok());
                if optiscaler.is_some_and(|value| value != "0" && !value.is_empty()) {
                    return Err(lapinux_core::Error::Other(
                        "afmf-linux et OptiScaler ne doivent pas être activés ensemble".into(),
                    ));
                }
                let version = game
                    .settings
                    .afmf_version
                    .as_deref()
                    .unwrap_or(&config.graphics.afmf_version);
                if !lapinux_core::config::AFMF_VERSIONS.contains(&version)
                    && version != lapinux_core::config::AFMF_LOCAL_VERSION
                {
                    return Err(lapinux_core::Error::Other(
                        "unsupported afmf-linux version".into(),
                    ));
                }
                let dir = self
                    .paths
                    .tools_dir
                    .join("frame-generation/afmf-linux")
                    .join(version);
                crate::jobs::validate_afmf_runtime(&dir, version)
                    .map_err(lapinux_core::Error::Other)?;
                let mut layer_path = dir.to_string_lossy().to_string();
                let existing = std::env::var("VK_ADD_IMPLICIT_LAYER_PATH").ok();
                if let Some(existing) = existing.filter(|value| !value.is_empty()) {
                    layer_path.push(':');
                    layer_path.push_str(&existing);
                }
                frame_env.extend([
                    ("VK_ADD_IMPLICIT_LAYER_PATH".into(), Some(layer_path)),
                    (
                        "AFMF_PERFORMANCE_MODE".into(),
                        Some(game.settings.afmf_performance_mode.unwrap_or(config.graphics.afmf_performance_mode).wire().into()),
                    ),
                    (
                        "AFMF_SEARCH_MODE".into(),
                        Some(game.settings.afmf_search_mode.unwrap_or(config.graphics.afmf_search_mode).wire().into()),
                    ),
                ]);
                frame_env.push((
                    "AFMF_GAMESCOPE".into(),
                    gamescope_active.then(|| "1".into()),
                ));
                // Le multiplicateur vit dans un fichier privé de la session :
                // l'overlay le change à chaud, et la couche repart d'un
                // historique frais quand il repasse à 0.
                let multiplier = lapinux_core::config::afmf_multiplier_or_default(
                    game.settings
                        .afmf_multiplier
                        .unwrap_or(config.graphics.afmf_multiplier),
                );
                // La génération d'images double la charge : sans cadence de
                // base, la couche réclame le GPU sans plafond (mesuré à 99 %
                // contre 56 % sans génération) et les scènes lourdes
                // s'effondrent. DXVK reçoit donc une cadence réelle (plafond
                // divisé par le multiplicateur) qu'il respecte strictement ;
                // GameScope garde la main au-dessus (`debug_set_fps_limit`),
                // ce qui permet de baisser la limite à chaud.
                if let Some(paced) = afmf_paced_cap(perf_config.gamescope.fps_cap, multiplier) {
                    frame_env.push(("DXVK_FRAME_RATE".into(), Some(paced.to_string())));
                }
                let control = self.paths.log_dir.join(format!(
                    "afmf-{appid}-{}.control",
                    lapinux_core::util::now_unix()
                ));
                match write_afmf_multiplier(&control, multiplier) {
                    Ok(()) => {
                        frame_env.push((
                            "AFMF_LAPINUX_CONTROL".into(),
                            Some(control.to_string_lossy().into_owned()),
                        ));
                        afmf_multiplier = Some(multiplier);
                        afmf_control = Some(control);
                    }
                    Err(error) => tracing::warn!(
                        "afmf-linux : fichier de contrôle indisponible ({error}) ; multiplicateur ×2 sans réglage à chaud"
                    ),
                }
            }
        }
        if game.settings.vkbasalt.unwrap_or(config.graphics.vkbasalt) {
            frame_env.push(("ENABLE_VKBASALT".into(), Some("1".into())));
            let bits = game.exe.as_deref().and_then(game_bits);
            match lapinux_core::vkbasalt::ensure_launch_layer(&self.paths, bits) {
                Ok(Some(dir)) => add_implicit_layer_path(&mut frame_env, &dir),
                Ok(None) => {}
                Err(error) if lapinux_core::vkbasalt::system_installed() => {
                    tracing::warn!(
                        "runtime vkBasalt Lapinux indisponible ({error}), layer système utilisé"
                    );
                }
                Err(error) => {
                    return Err(lapinux_core::Error::Other(format!(
                        "runtime vkBasalt : {error}"
                    )))
                }
            }
            let effect = game
                .settings
                .vkbasalt_effect
                .unwrap_or(config.graphics.vkbasalt_effect);
            let preset = if effect != lapinux_core::config::VkBasaltEffect::Existing {
                Some(vkbasalt_preset(&self.paths.data_dir, appid, effect)?)
            } else {
                None
            };
            if let Some(path) = preset {
                frame_env.push((
                    "VKBASALT_CONFIG_FILE".into(),
                    Some(path.to_string_lossy().into_owned()),
                ));
            } else if !config.graphics.vkbasalt_config.trim().is_empty() {
                let path = lapinux_core::paths::expand_tilde(&config.graphics.vkbasalt_config);
                if !path.is_file() {
                    return Err(lapinux_core::Error::Other(
                        "vkBasalt configuration file not found".into(),
                    ));
                }
                frame_env.push((
                    "VKBASALT_CONFIG_FILE".into(),
                    Some(path.to_string_lossy().into_owned()),
                ));
            }
        }
        // Ne pas écraser une configuration explicitement fournie par le joueur.
        if let Some((key, _)) = frame_env.iter().find(|(key, _)| {
            !key.eq_ignore_ascii_case("WINEDLLOVERRIDES")
                && game
                    .settings
                    .env
                    .iter()
                    .any(|(existing, _)| existing.eq_ignore_ascii_case(key))
        }) {
            return Err(lapinux_core::Error::Other(format!(
                "graphics: conflicting game environment variable {key}"
            )));
        }
        frame_env.extend(compatibility);
        if let Some(preload) = native_preload {
            let data_dir = self.paths.data_dir.to_str().ok_or_else(|| {
                lapinux_core::Error::Other(
                    "pont Steamworks : le dossier de données doit avoir un chemin UTF-8".into(),
                )
            })?;
            frame_env.push(("LAPINUX_STEAMSHIM_DATA".into(), Some(data_dir.to_string())));
            frame_env.push(("LD_PRELOAD".into(), Some(preload)));
        }
        perf_config.game_env = frame_env;
        let mut prepared = lapinux_perf::prepare(
            &plan,
            appid,
            &perf_config,
            &caps,
            &self.paths.config_dir,
            &self.paths.log_dir,
        )
        .map_err(|e| lapinux_core::Error::Other(e.to_string()))?;
        // L'utilisateur peut avoir besoin de ses propres préchargements dans
        // GameScope aussi ; seul le pont est réservé à l'arbre du jeu.
        if let Some(previous) = native_parent_preload {
            prepared.command.env("LD_PRELOAD", previous);
        }
        let summary = lapinux_perf::layers_summary(prepared.gamescope);

        if dry_run {
            return Ok(Envelope::ok(&serde_json::json!({
                "command": lapinux_perf::render_prepared(&prepared),
                "proton_plan": lapinux_proton::render_plan(&plan),
                "layers": summary,
                "gamescope": prepared.gamescope,
                // Pourquoi GameScope a été écarté, le cas échéant : la même
                // phrase est écrite dans le journal de la partie.
                "gamescope_warning": prepared.gamescope_warning,
                "log": prepared.log_path,
                "drm": game.drm,
                "proton": proton.as_ref().map(|p| p.name.clone()),
                "steam_client": false,
                "shader_cache": shader_cache
                    .as_ref()
                    .map(|path| path.display().to_string()),
                "shader_cache_vars": shader_cache_vars,
            })));
        }

        // Les sauvegardes du compte, avant que le jeu ne démarre : sans client
        // Steam, rien ne les a jamais posées dans le préfixe. Seul ce que le
        // cloud a de plus récent que le disque est repris — une progression
        // déjà jouée reste —, et un échec ne bloque pas le lancement. Un jeu
        // de boutique n'a pas de cloud Steam : son appid Lapinux n'en désigne
        // aucun, la lecture est donc sautée.
        if let Ok(mut baseline) = self.store_cloud_before.lock() {
            *baseline = None;
        }
        if !game.settings.cloud_sync_enabled() {
            tracing::debug!("cloud : synchronisation automatique désactivée pour {appid}");
        } else if steam_game {
            match self.sync_cloud(&game, appid, true) {
                Ok((0, _)) => {}
                Ok((files, bytes)) => {
                    tracing::info!(
                        "cloud : {files} sauvegarde(s) reprise(s) avant la partie de {appid} ({bytes} octets)"
                    );
                    if let Ok(mut cache) = self.game_reads.lock() {
                        cache.forget("cloud", appid);
                    }
                    self.forget_remote_sheet("cloud", appid);
                }
                Err(error)
                    if error.contains("sauvegardes différentes")
                        || error.contains("empreinte cloud indisponible")
                        || error.contains("chemin de sauvegarde inconnu") =>
                {
                    tracing::warn!("cloud : lancement interrompu pour protéger les sauvegardes de {appid} ({error})");
                    return Err(lapinux_core::Error::Other(format!("sauvegardes Steam : {error}. Vérifiez l'onglet Sauvegardes avant de jouer.")));
                }
                Err(error) => {
                    tracing::warn!(
                        "cloud : synchronisation indisponible avant la partie de {appid} ({error})"
                    );
                }
            }
        } else if game.settings.save_dir.is_some() {
            // Jeu de boutique : le dossier des sauvegardes est celui que
            // l'utilisateur a désigné. Sa boutique a pu changer depuis la
            // dernière partie, on reprend ce qu'elle a de plus récent.
            match self.store_cloud_pull_auto(appid) {
                Ok((files, bytes)) => {
                    if files > 0 {
                        tracing::info!(
                            "cloud {} : {files} sauvegarde(s) reprise(s) avant la partie de {appid} ({bytes} octets)",
                            game.store.label()
                        );
                    }
                    if let Err(error) = self.capture_store_cloud_baseline(appid) {
                        tracing::warn!("cloud {} : relevé avant partie indisponible, envoi automatique désactivé ({error})", game.store.label());
                    }
                }
                Err(error) if error.contains("sauvegardes différentes") => {
                    return Err(lapinux_core::Error::Other(format!(
                        "sauvegardes {} : {error}. Vérifiez l'onglet Sauvegardes avant de jouer.",
                        game.store.label()
                    )));
                }
                Err(error) => tracing::warn!(
                    "cloud {} : rien repris avant la partie de {appid} ({error})",
                    game.store.label()
                ),
            }
        }

        // Lapinux ne démarre pas de launcher tiers. Un DRM ou un composant
        // imposé par le jeu peut néanmoins empêcher son lancement direct.
        // La fenêtre de gamescope apparaît avant la première image du jeu :
        // sa présence seule ne suffit pas à retirer l'écran d'attente.
        let gamescope = prepared.gamescope;
        let scene = prepared.scene;
        // Image publique pour le Rich Presence : l'en-tête Steam est stable,
        // les boutiques publient leur jaquette dans leur bibliothèque.
        let cover = if game.store == lapinux_core::stores::Store::Steam {
            format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/header.jpg")
        } else {
            self.cached_store_entry(appid)
                .map(|entry| entry.cover)
                .unwrap_or_default()
        };
        // Application Discord du jeu : avec elle, l'activité porte le nom et
        // l'icône du jeu au lieu de « Lapinux ». Vide si Discord ne connaît
        // pas ce jeu.
        let discord_app = self.discord_app_for(&game);
        // Pont Windows : le DLL de Lapinux prend la place de celui du jeu
        // pendant la partie, puis l'original est remis en place.
        let swap = if !game.native && game.store == lapinux_core::stores::Store::Steam {
            crate::steam_shim::install_steam_shim(
                &game.install_dir(),
                &self.paths.data_dir,
                appid,
                game.settings.steam_shim,
            )
        } else {
            None
        };
        // Le service d'entrée remplace les manettes par son pad virtuel. Si
        // ses sources restent visibles, Wine/SDL peut choisir le pont ou le
        // Steam Controller grabbé et le jeu ne reçoit plus rien. Le masquage
        // n'a de sens que si ce service va réellement démarrer.
        let wants_mapping = game.settings.grab_devices(config.input.grab_devices)
            || game.settings.input_profile.is_some()
            || self.active_steam_input_profile(appid).is_some();
        let input_profile = game
            .settings
            .input_profile
            .clone()
            .unwrap_or_else(|| config.input.default_profile.clone());
        let wants_pad_hotkey = !config.overlay.pad_hotkey.trim().is_empty();
        // Le service et son pad virtuel naissent avant le jeu. Sinon bwrap a
        // déjà masqué les manettes physiques quand le jeu énumère les
        // périphériques : il ne voit rien, et les titres qui n'ajoutent pas de
        // manette à chaud restent sans entrée pour toute la partie.
        let input = if wants_mapping || wants_pad_hotkey {
            self.prepare_input(appid, &input_profile, wants_mapping)
        } else {
            None
        };
        // Si le pad virtuel n'a pas pu démarrer, les sources restent visibles :
        // une manette brute vaut mieux qu'un jeu sans aucune manette.
        if wants_mapping {
            if let Some(service) = &input {
                hide_input_sources(&mut prepared, service.initial_sources());
            }
        }
        let session = match lapinux_perf::Session::spawn(appid, prepared) {
            Ok(session) => session,
            Err(error) => {
                if let Some(swap) = swap {
                    swap.restore();
                }
                return Err(lapinux_core::Error::Other(error.to_string()));
            }
        };
        if let Some(swap) = swap {
            let pid = session.pid;
            std::thread::spawn(move || {
                // `/proc/<pid>` survit à un processus zombie : sans ce test, un
                // GameScope non récolté garderait le pont posé pour toujours, et
                // le lancement suivant le prendrait pour un pont déjà en place.
                while crate::captures::process_is_alive(pid) {
                    std::thread::sleep(Duration::from_secs(2));
                }
                swap.restore();
            });
        }
        let info = SessionInfo {
            appid,
            name: game.name.clone(),
            pid: session.pid,
            started: lapinux_core::util::now_unix(),
            proton: proton.map(|p| p.name).unwrap_or_else(|| "natif".into()),
            fps_cap: perf_config.gamescope.fps_cap,
            // Fréquence réelle de l'écran, pas la limite choisie : l'overlay
            // s'en sert pour ne pas proposer une cadence que le moniteur ne
            // peut pas afficher.
            refresh_hz: if gamescope {
                perf_config.gamescope.refresh_hz
            } else {
                0
            },
            overlay_open: false,
            input_profile: input_profile.clone(),
            log_path: session.log_path.clone(),
            window_shown: false,
            // Hors gamescope, la fenêtre du jeu est celle du jeu lui-même :
            // elle n'apparaît qu'une fois son rendu prêt. Sous gamescope, le
            // guetteur du journal décidera.
            render_started: !gamescope && !scene,
            window_gone: false,
            gamescope,
            scene,
            performance: game.settings.performance,
            cover,
            discord_app,
            fps: None,
            fps_series: Vec::new(),
            fps_series_start: 0,
            fps_series_end: 0,
            store: game.store,
            dlss: {
                // Le relevé complet sert la fiche ; la session ne garde que
                // les bibliothèques DLSS, et rien quand il n'y en a pas.
                let tech = crate::steam::detect_tech(&game.install_dir());
                (!tech.dlss.is_empty()).then_some(tech.dlss)
            },
            afmf_multiplier,
            afmf_control,
        };
        // Une sélection Steam Input active doit aussi activer le service même
        // si le jeu n'a pas encore de profil TOML enregistré.
        self.adopt_session(info.clone(), session, input);
        // Un appid Lapinux n'est pas un appid Steam : seuls les jeux de Steam
        // peuvent apparaître « en jeu » sur le profil.
        self.set_steam_playing((game.store == lapinux_core::stores::Store::Steam).then_some(appid));

        // Relevé du cloud avant que le jeu n'écrive : à sa sortie, la
        // comparaison dira ce que la partie a envoyé. La lecture passe par la
        // session partagée, dans un fil à part pour ne pas retarder le
        // lancement.
        if let Ok(mut slot) = self.cloud_last.lock() {
            if slot.as_ref().map(|(known, _)| *known) == Some(appid) {
                *slot = None;
            }
        }
        // Le relevé du cloud n'a de sens que pour un jeu Steam.
        let shared = Arc::clone(self);
        if !steam_game {
            return Ok(Envelope::ok(&serde_json::json!({
                "session": info,
                "layers": summary,
                "steam_client": false,
            })));
        }
        std::thread::spawn(move || {
            let Some(saves) = shared.read_cloud(appid) else {
                return;
            };
            tracing::info!(
                "cloud : {} fichier(s) avant la partie de {appid}",
                saves.files.len()
            );
            if let Ok(mut slot) = shared.cloud_before.lock() {
                *slot = Some((appid, saves));
            }
        });

        Ok(Envelope::ok(&serde_json::json!({
            "session": info,
            "layers": summary,
            "steam_client": false,
        })))
    }
}

#[cfg(test)]
mod shader_cache_tests {
    use super::*;

    #[test]
    fn les_caches_de_shaders_vont_dans_un_dossier_court_de_la_bibliotheque() {
        let racine = tempfile::tempdir().unwrap();
        let bibliotheque = racine.path().join("LapinuxLibrary");
        let (dossier, variables) =
            shader_cache_environment(&bibliotheque, "Gabriel Knight (620)", &[]);
        let dossier = dossier.expect("le dossier doit être créé au lancement");
        assert!(dossier.is_dir());
        assert_eq!(
            dossier,
            bibliotheque.join("Shaders").join("Gabriel Knight (620)")
        );
        assert_eq!(variables.len(), SHADER_CACHE_VARIABLES.len());
        for (cle, valeur) in &variables {
            assert!(SHADER_CACHE_VARIABLES.contains(&cle.as_str()));
            assert_eq!(valeur, &dossier.display().to_string());
        }
    }

    #[test]
    fn les_variables_de_shaders_du_joueur_ne_sont_jamais_ecrasees() {
        let racine = tempfile::tempdir().unwrap();
        let joueur = vec![(
            "MESA_SHADER_CACHE_DIR".to_string(),
            "/tmp/cache-du-joueur".to_string(),
        )];
        let (dossier, variables) = shader_cache_environment(racine.path(), "Essai (7)", &joueur);
        assert_eq!(dossier.unwrap(), racine.path().join("Shaders").join("Essai (7)"));
        assert_eq!(variables.len(), SHADER_CACHE_VARIABLES.len() - 1);
        assert!(!variables
            .iter()
            .any(|(cle, _)| cle == "MESA_SHADER_CACHE_DIR"));
    }
}
