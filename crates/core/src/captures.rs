//! Noms et dossiers des captures.
//!
//! Le nom d'une vidéo est un modèle lisible, mais il reste un nom de fichier :
//! les séparateurs de chemin et les caractères interdits sont neutralisés avant
//! l'écriture. Le même module relit les noms pour que la galerie retrouve le
//! jeu et l'heure sans dépendre d'un index séparé.

use std::path::PathBuf;

use crate::paths::{expand_tilde, home_dir};

/// Modèle par défaut : date ISO, heure locale, jeu puis cadence.
pub const DEFAULT_NAME_FORMAT: &str = "{date} {time} - {jeu} - {fps} FPS";

/// Marqueur discret qui distingue un dossier de captures d'un dossier de
/// vidéos quelconque. Il n'apparaît pas dans la galerie.
pub const MARKER: &str = ".lapinux-captures";

/// Résout un dossier saisi dans les réglages. Vide veut dire « dossier XDG par
/// défaut » ; `~` et les chemins relatifs sont acceptés.
pub fn resolve_dir(value: &str, fallback: impl FnOnce() -> PathBuf) -> PathBuf {
    let value = value.trim();
    if value.is_empty() {
        return fallback();
    }
    let path = expand_tilde(value);
    if path.is_absolute() {
        path
    } else {
        home_dir().join(path)
    }
}

/// Nettoie un composant de nom de fichier sans perdre les accents ni les
/// espaces lisibles. Les séparateurs sont remplacés, jamais interprétés.
pub fn sanitize_name(value: &str) -> String {
    let mut out = String::new();
    let mut space = false;
    for c in value.chars() {
        let c = match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => ' ',
            c => c,
        };
        if c == ' ' {
            if !out.is_empty() {
                space = true;
            }
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
    }
    out.trim_matches(|c: char| c == '.' || c == ' ')
        .chars()
        .take(180)
        .collect()
}

/// Rend le nom sans extension à partir du modèle choisi par l'utilisateur.
pub fn render_stem(
    template: &str,
    game: &str,
    appid: u32,
    fps: u32,
    at: i64,
) -> String {
    let date = iso_date(at);
    let time = file_time(at);
    let game = sanitize_name(game);
    let fps = fps.to_string();
    let appid = appid.to_string();
    let unix = at.to_string();
    let values = [
        ("{date}", date.as_str()),
        ("{time}", time.as_str()),
        ("{jeu}", game.as_str()),
        ("{fps}", fps.as_str()),
        ("{appid}", appid.as_str()),
        ("{unix}", unix.as_str()),
    ];
    let mut out = template.to_string();
    for (token, value) in values {
        out = out.replace(token, value);
    }
    let out = sanitize_name(&out);
    if out.is_empty() {
        format!("Capture {date} {time}")
    } else {
        out
    }
}

/// Date locale d'un horodatage, dans l'ordre qui se trie tout seul.
pub fn iso_date(unix: i64) -> String {
    match local_tm(unix) {
        Some(tm) => format!(
            "{:04}-{:02}-{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday
        ),
        None => "date-inconnue".into(),
    }
}

/// Heure locale sans deux-points : un nom de fichier reste copiable partout.
pub fn file_time(unix: i64) -> String {
    match local_tm(unix) {
        Some(tm) => format!("{:02}-{:02}-{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec),
        None => "heure-inconnue".into(),
    }
}

/// Retrouve l'horodatage inscrit dans un nom, même entouré de texte.
pub fn parse_stamp(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    for start in 0..bytes.len().saturating_sub(18) {
        let Some(slice) = bytes.get(start..start + 19) else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(slice) else {
            continue;
        };
        let Some(stamp) = parse_stamp_text(text) else {
            continue;
        };
        return Some(stamp);
    }
    None
}

/// Relit le jeu dans un nom produit par Lapinux. Les dates, heures et
/// indications de cadence sont retirées ; le reste est le titre du jeu.
pub fn game_from_stem(stem: &str) -> String {
    let mut rest = stem.to_string();
    if let Some((start, end)) = stamp_range(stem) {
        rest.replace_range(start..end, " ");
    }
    let mut words = Vec::new();
    let tokens: Vec<&str> = rest.split_whitespace().collect();
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index].trim_matches(|c: char| matches!(c, '-' | '_' | '(' | ')' | '[' | ']'));
        let next = tokens
            .get(index + 1)
            .map(|next| next.trim_matches(|c: char| matches!(c, '-' | '_' | '(' | ')' | '[' | ']')));
        let is_number = token.chars().all(|c| c.is_ascii_digit()) && !token.is_empty();
        let is_rate = matches!(next, Some("FPS" | "fps" | "IPS" | "ips"));
        if is_number && is_rate {
            index += 2;
            continue;
        }
        if token.is_empty() || matches!(token, "-" | "_" | "|" | "(" | ")" | "[" | "]") {
            index += 1;
            continue;
        }
        words.push(token);
        index += 1;
    }
    let game = words.join(" ").trim_matches(|c: char| matches!(c, '-' | '_' | '(' | ')' | '[' | ']' | ' ')).to_string();
    if game.is_empty() {
        stem.trim_matches(|c: char| matches!(c, '-' | '_' | '(' | ')' | '[' | ']' | ' ')).to_string()
    } else {
        game
    }
}

fn local_tm(unix: i64) -> Option<libc::tm> {
    if unix <= 0 {
        return None;
    }
    let stamp = unix as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { !libc::localtime_r(&stamp, &mut tm).is_null() };
    ok.then_some(tm)
}

fn parse_stamp_text(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() != 19
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b' '
        || bytes[13] != b'-'
        || bytes[16] != b'-'
        || !bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7 | 10 | 13 | 16) || byte.is_ascii_digit())
    {
        return None;
    }
    let number = |start: usize, end: usize| text[start..end].parse::<i32>().ok();
    let year = number(0, 4)?;
    let month = number(5, 7)?;
    let day = number(8, 10)?;
    let hour = number(11, 13)?;
    let minute = number(14, 16)?;
    let second = number(17, 19)?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..24).contains(&hour)
        || !(0..60).contains(&minute)
        || !(0..60).contains(&second)
    {
        return None;
    }
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = year - 1900;
    tm.tm_mon = month - 1;
    tm.tm_mday = day;
    tm.tm_hour = hour;
    tm.tm_min = minute;
    tm.tm_sec = second;
    tm.tm_isdst = -1;
    let stamp = unsafe { libc::mktime(&mut tm) };
    (stamp >= 0).then_some(stamp as i64)
}

fn stamp_range(value: &str) -> Option<(usize, usize)> {
    let bytes = value.as_bytes();
    for start in 0..bytes.len().saturating_sub(18) {
        let Some(slice) = bytes.get(start..start + 19) else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(slice) else {
            continue;
        };
        if parse_stamp_text(text).is_some() {
            return Some((start, start + 19));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_modele_par_defaut_donne_un_nom_lisible() {
        let at = 1_793_000_000;
        let stem = render_stem(DEFAULT_NAME_FORMAT, "MotorSlice", 0, 30, at);
        assert!(stem.ends_with(" - MotorSlice - 30 FPS"), "{stem}");
        assert_eq!(stem, format!("{} {} - MotorSlice - 30 FPS", iso_date(at), file_time(at)));
    }

    #[test]
    fn les_separateurs_ne_sortent_jamais_du_dossier() {
        let stem = render_stem("{date} - {jeu} - {fps} FPS", "../Motor/Slice", 0, 30, 1_793_000_000);
        assert!(!stem.contains('/'));
        assert!(stem.contains("Motor-Slice"));
    }

    #[test]
    fn un_nom_se_relit_sans_son_horodatage() {
        let at = 1_793_000_000;
        let stem = render_stem(DEFAULT_NAME_FORMAT, "MotorSlice", 0, 30, at);
        assert_eq!(game_from_stem(&stem), "MotorSlice");
        assert_eq!(parse_stamp(&stem), Some(at));
    }

    #[test]
    fn un_dossier_vide_suit_le_defaut() {
        let path = resolve_dir("", || PathBuf::from("/tmp/lapinux-videos"));
        assert_eq!(path, PathBuf::from("/tmp/lapinux-videos"));
    }
}
