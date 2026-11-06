//! Petits utilitaires partagés.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Copie un dossier fichier par fichier, liens symboliques compris : un
/// préfixe Proton en dépend (`dosdevices`, `pfx`), et une sauvegarde copiée
/// sans ses liens ne serait pas restaurable.
pub fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            let link = std::fs::read_link(entry.path())?;
            std::os::unix::fs::symlink(link, &target)?;
        } else if file_type.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Taille au-delà de laquelle un journal est tourné. Huit mégaoctets
/// gardent des centaines de milliers de lignes, largement de quoi retrouver
/// la cause d'un incident, sans laisser un fichier grossir sans fin.
pub const LOG_CAP: u64 = 8 * 1024 * 1024;

/// Tourne un journal devenu trop gros : il part en `.1` (l'ancien `.1` est
/// effacé) et un fichier neuf commence. Un journal ouvert en ajout seul finit
/// par peser des dizaines de mégaoctets, comme `downloads.log` après quelques
/// installations.
pub fn rotate_log(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if meta.len() <= LOG_CAP {
        return false;
    }
    let backup = PathBuf::from(format!("{}.1", path.display()));
    let _ = std::fs::remove_file(&backup);
    if std::fs::rename(path, &backup).is_err() {
        // Un renommage impossible (autre système de fichiers) ne doit pas
        // empêcher le journal de repartir : on le vide sur place.
        let _ = std::fs::write(path, b"");
    }
    true
}

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Taille lisible en base 1000 (Ko, Mo, Go), la convention des tailles de
/// jeux et des débits affichés par Steam et les navigateurs — et la virgule
/// décimale unique de l'interface.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["o", "Ko", "Mo", "Go", "To"];
    if bytes < 1000 {
        return format!("{bytes} o");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    // Une seule convention decimale dans toute l'interface : la virgule.
    let number = if value >= 100.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    };
    format!("{} {}", number.replace('.', ","), UNITS[unit])
}

/// Capacité en Gio, pour la mémoire vive et la mémoire vidéo.
///
/// Le Go décimal gonfle les totaux (33,5 Go pour une barrette de 32 Go,
/// 12,9 Go pour une carte de 12 Go) : la capacité installée se lit en Gio,
/// comme l'annoncent les fabricants.
pub fn format_gib(bytes: u64) -> String {
    let value = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    let number = if value >= 100.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    };
    format!("{} Gio", number.replace('.', ","))
}

/// Famille d'un build Proton : ce qui reste de son nom avant le premier
/// chiffre, en minuscules et sans séparateurs. « GE-Proton11-7 » et
/// « ge-proton11-9 » sont de la même famille.
pub fn proton_family(name: &str) -> String {
    let cut = name
        .find(|c: char| c.is_ascii_digit())
        .unwrap_or(name.len());
    name[..cut]
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase()
}

/// Famille d'un build, le suffixe « latest » retiré : « DW-Proton Latest »
/// appartient à la famille de « dwproton-11.0-15 », et un build « latest »
/// remplace donc l'ancienne version numérotée.
pub fn proton_family_key(name: &str) -> String {
    let family = proton_family(name);
    family
        .strip_suffix("latest")
        .unwrap_or(&family)
        .to_string()
}

/// Horloge locale d'un instant : `localtime_r` tient compte du fuseau et de
/// l'heure d'été, ce que l'arithmétique sur l'horodatage brut ignore. Une date
/// calculée en UTC décalait d'un jour les soirées à l'est de Greenwich — un
/// en-tête « Aujourd'hui » pouvait annoncer la veille.
fn local_tm(unix: i64) -> Option<libc::tm> {
    if unix <= 0 {
        return None;
    }
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let stamp = unix as libc::time_t;
    // SAFETY: `localtime_r` écrit dans `tm`, fourni par l'appelant, et ne
    // conserve aucun état partagé entre les appels.
    let ok = unsafe { !libc::localtime_r(&stamp, &mut tm).is_null() };
    ok.then_some(tm)
}

/// Date locale `JJ/MM/AAAA` : le jour où l'utilisateur a vécu l'événement, pas
/// celui de Greenwich.
pub fn format_date(unix: i64) -> String {
    let Some(tm) = local_tm(unix) else {
        return "—".into();
    };
    format!(
        "{:02}/{:02}/{:04}",
        tm.tm_mday,
        tm.tm_mon + 1,
        tm.tm_year + 1900
    )
}

/// Date et heure locales `JJ/MM/AAAA à HH:MM` : pour une tâche, l'heure de
/// démarrage compte autant que le jour.
pub fn format_datetime(unix: i64) -> String {
    let Some(tm) = local_tm(unix) else {
        return "—".into();
    };
    format!(
        "{:02}/{:02}/{:04} à {:02}:{:02}",
        tm.tm_mday,
        tm.tm_mon + 1,
        tm.tm_year + 1900,
        tm.tm_hour,
        tm.tm_min
    )
}

/// Heure locale `HH:MM` : les cartes d'activité datent le lancement et la
/// fermeture d'une partie sans répéter la date du jour.
pub fn format_time(unix: i64) -> String {
    let Some(tm) = local_tm(unix) else {
        return "—".into();
    };
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}

/// Heure locale `HH:MM:SS` : l'horloge de la barre du haut avance à la
/// seconde près, sans attendre la minute suivante.
pub fn format_time_seconds(unix: i64) -> String {
    let Some(tm) = local_tm(unix) else {
        return "—".into();
    };
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}

/// Jour d'une partie, nommé quand il est proche : « Aujourd'hui », « Hier »,
/// puis la date. Sert d'en-tête repliable à l'historique.
pub fn day_label(unix: i64, now: i64) -> String {
    if unix <= 0 {
        return "Date inconnue".into();
    }
    let (day, today) = (day_start(unix), day_start(now));
    let date = format_date(unix);
    if day == today {
        format!("Aujourd'hui · {date}")
    } else if day == day_start(now - 86_400) {
        format!("Hier · {date}")
    } else {
        date
    }
}

/// Minuit local d'un instant, en secondes Unix : deux parties du même jour
/// doivent tomber sur la même clé.
pub fn day_start(unix: i64) -> i64 {
    if unix <= 0 {
        return 0;
    }
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let stamp = unix as libc::time_t;
    // SAFETY: comme ci-dessus, `localtime_r` ne partage aucun état.
    if unsafe { libc::localtime_r(&stamp, &mut tm) }.is_null() {
        return 0;
    }
    tm.tm_hour = 0;
    tm.tm_min = 0;
    tm.tm_sec = 0;
    // `mktime` normalise les champs et tient compte du fuseau et de l'heure
    // d'été ; `-1` signale une date impossible.
    let midnight = unsafe { libc::mktime(&mut tm) };
    if midnight < 0 {
        0
    } else {
        midnight as i64
    }
}

pub fn format_duration(seconds: u64) -> String {
    let h = seconds / 3600;
    let m = (seconds % 3600) / 60;
    if h > 0 {
        format!("{h} h {m:02}")
    } else if m > 0 {
        format!("{m} min")
    } else {
        format!("{seconds} s")
    }
}

/// Durée d'allumage de la machine : les jours comptent, sinon une machine
/// restée une semaine allumée afficherait « 168 h ».
pub fn format_uptime(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3600;
    let minutes = (seconds % 3600) / 60;
    if days > 0 {
        if hours > 0 {
            format!("{days} j {hours:02} h")
        } else {
            format!("{days} j")
        }
    } else if hours > 0 {
        format!("{hours} h {minutes:02}")
    } else {
        format!("{minutes} min")
    }
}

pub fn ellipsize(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Slug utilisé pour les noms de dossier (protons, profils).
pub fn slugify(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_dash = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "sans-nom".into()
    } else {
        trimmed
    }
}

pub fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proton_families_gather_the_same_builds() {
        assert_eq!(proton_family("GE-Proton11-7"), "geproton");
        assert_eq!(proton_family("ge-proton11-9"), "geproton");
        assert_eq!(proton_family("Proton - Experimental"), "protonexperimental");
        // Un « Latest » rejoint la famille de sa version numérotée.
        assert_eq!(proton_family_key("DW-Proton Latest"), "dwproton");
        assert_eq!(proton_family_key("dwproton-11.0-15-x86_64"), "dwproton");
        // Et ne se confond pas avec un autre build de Valve.
        assert_ne!(proton_family_key("Proton 11.0"), proton_family_key("Proton - Experimental"));
    }

    #[test]
    fn bytes_units() {
        assert_eq!(format_bytes(512), "512 o");
        assert_eq!(format_bytes(2048), "2,0 Ko");
        assert_eq!(format_bytes(50 * 1024 * 1024), "52,4 Mo");
        assert_eq!(format_bytes(700 * 1024 * 1024 * 1024), "752 Go");
        // Base 1000 : aucun libellé binaire ne doit subsister.
        assert!(!format_bytes(1024).contains("Kio"));
        assert_eq!(format_bytes(1_000_000_000), "1,0 Go");
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(42), "42 s");
        assert_eq!(format_duration(125), "2 min");
        assert_eq!(format_duration(7300), "2 h 01");
        assert_eq!(format_uptime(30), "0 min");
        assert_eq!(format_uptime(7300), "2 h 01");
        assert_eq!(format_uptime(273_900), "3 j 04 h");
        let clock = format_time_seconds(1_700_000_000);
        assert_eq!(clock.len(), 8, "{clock}");
        assert_eq!(clock.matches(':').count(), 2, "{clock}");
    }

    #[test]
    fn slugs() {
        assert_eq!(slugify("GE-Proton 11.7"), "ge-proton-11-7");
        assert_eq!(slugify("!!!"), "sans-nom");
    }

    #[test]
    fn ellipsis() {
        assert_eq!(ellipsize("abcdef", 4), "abc…");
        assert_eq!(ellipsize("abc", 10), "abc");
    }

    #[test]
    fn dates_are_readable() {
        // Midi UTC : la date locale ne dépend pas du fuseau de la machine qui
        // fait tourner le test.
        assert_eq!(format_date(1_192_017_600), "10/10/2007");
        assert_eq!(format_date(43_200), "01/01/1970");
    }

    /// Une soirée à l'est de Greenwich garde son jour : la date se lit sur
    /// l'horloge locale, pas sur l'horodatage brut.
    #[test]
    fn the_day_label_matches_the_local_day() {
        let now = 1_791_000_000;
        // L'en-tête reçoit minuit local : sa date doit être celle du jour.
        let label = day_label(day_start(now), now);
        assert!(label.starts_with("Aujourd'hui"), "{label}");
    }

    /// La copie garde les liens symboliques : un préfixe ou une sauvegarde
    /// qui en contient doit rester utilisable.
    #[test]
    fn copy_tree_keeps_symlinks_and_files() {
        let dir = std::env::temp_dir().join(format!("lapinux-copy-{}", now_unix()));
        let _ = std::fs::remove_dir_all(&dir);
        let from = dir.join("from");
        std::fs::create_dir_all(from.join("a/b")).unwrap();
        std::fs::write(from.join("a/b/reg"), b"x").unwrap();
        std::os::unix::fs::symlink("reg", from.join("a/b/lien")).unwrap();
        let to = dir.join("to");
        copy_tree(&from, &to).unwrap();
        assert_eq!(std::fs::read(to.join("a/b/reg")).unwrap(), b"x");
        assert_eq!(
            std::fs::read_link(to.join("a/b/lien")).unwrap(),
            PathBuf::from("reg")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_oversized_log_is_rotated() {
        let dir = std::env::temp_dir().join(format!("lapinux-rotate-{}", now_unix()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("daemon.log");
        std::fs::write(&log, vec![b'x'; LOG_CAP as usize + 1]).unwrap();
        assert!(rotate_log(&log));
        assert!(!log.exists());
        let backup = dir.join("daemon.log.1");
        assert_eq!(std::fs::metadata(&backup).unwrap().len(), LOG_CAP + 1);
        // Un journal déjà petit reste en place, sans copie.
        std::fs::write(&log, "court").unwrap();
        assert!(!rotate_log(&log));
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "court");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn datetimes_include_the_local_clock() {
        let text = format_datetime(1_191_999_600);
        assert!(text.contains(" à "), "{text}");
        assert!(text.contains(':'), "{text}");
        assert_eq!(format_datetime(0), "—");
    }
}
