//! Ce que les trois boutiques partagent : réseau, plan d'installation,
//! extraction et écriture du manifeste Lapinux.

use lapinux_core::stores::Store;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, Error>;

/// Erreur d'une boutique, assez précise pour que l'utilisateur sache quoi
/// faire : se reconnecter, réessayer plus tard, ou constater que la boutique
/// ne propose pas la fonction.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("réseau : {0}")]
    Http(String),
    #[error("réponse inattendue de la boutique : {0}")]
    Parse(String),
    #[error("session expirée : reconnecte la boutique")]
    Expired,
    #[error("la boutique a refusé ({status}) : {message}")]
    Refused { status: u16, message: String },
    #[error("{0}")]
    Unavailable(&'static str),
    #[error("extraction impossible : {0}")]
    Archive(String),
    #[error("E/S : {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

impl From<ureq::Error> for Error {
    fn from(error: ureq::Error) -> Self {
        match error {
            ureq::Error::Status(status, response) => {
                let message = response.into_string().unwrap_or_default();
                let message = message.chars().take(240).collect::<String>();
                if status == 401 || status == 403 {
                    Error::Expired
                } else {
                    Error::Refused { status, message }
                }
            }
            ureq::Error::Transport(transport) => Error::Http(transport.to_string()),
        }
    }
}

/// En-tête commun : les boutiques aiment savoir qui parle.
pub const USER_AGENT: &str = concat!("lapinux/", env!("CARGO_PKG_VERSION"));

const TIMEOUT: u64 = 30;

/// Une connexion demandée : l'adresse à ouvrir, et ce qu'il faut en rapporter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginLink {
    pub url: String,
    /// Ce que l'utilisateur doit coller après s'être connecté.
    pub hint: String,
}

/// Ce qu'un plan d'installation demande : des téléchargements, puis des
/// extractions ou un installeur à lancer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Télécharger `url` dans le cache sous `file`.
    Download { url: String, file: String, size: u64 },
    /// Extraire `file` (zip) dans le dossier d'installation.
    Unzip { file: String },
    /// Lancer l'installeur GOG (mojosetup) dans le dossier d'installation.
    RunInstaller { file: String },
    /// Extraire un installeur Windows GOG avec `innoextract`.
    InnoExtract { file: String },
    /// Télécharger un build Epic : `url` est son manifeste binaire, `size` le
    /// poids des chunks. Les fichiers sont recollés depuis les chunks du CDN.
    EpicBuild { url: String, size: u64 },
}

/// Tout ce qu'il faut pour installer un jeu de boutique.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPlan {
    pub store: Store,
    pub store_id: String,
    pub name: String,
    /// Nom du dossier sous `steamapps/common`.
    pub installdir: String,
    /// Poids annoncé, tous fichiers compris.
    pub size: u64,
    /// Poids sur le disque une fois installé, quand la boutique le publie :
    /// zéro pour un installeur ou une archive dont le contenu n'est pas connu
    /// avant décompression.
    pub installed: u64,
    pub platform: String,
    /// Identité stable du build publié (Epic : version et empreintes des
    /// chunks). Vide quand la boutique ne fournit pas d'identité stable.
    pub build: String,
    pub steps: Vec<Step>,
}

impl InstallPlan {
    /// Poids à télécharger : la somme des fichiers du plan.
    pub fn download_size(&self) -> u64 {
        self.steps
            .iter()
            .map(|step| match step {
                Step::Download { size, .. } => *size,
                Step::EpicBuild { size, .. } => *size,
                _ => 0,
            })
            .sum()
    }
}

/// GET simple, avec en-tête d'autorisation facultatif.
pub fn get(url: &str, token: Option<&str>) -> Result<String> {
    get_as(url, token, USER_AGENT)
}

/// GET sous l'identité d'un autre client : certains services d'Epic refusent
/// un agent inconnu (403) alors que le leur répond.
pub fn get_as(url: &str, token: Option<&str>, agent: &str) -> Result<String> {
    let mut request = ureq::get(url)
        .set("User-Agent", agent)
        .timeout(std::time::Duration::from_secs(TIMEOUT));
    let bearer = token.map(|token| format!("Bearer {token}"));
    if let Some(bearer) = &bearer {
        request = request.set("Authorization", bearer);
    }
    let response = request.call()?;
    response
        .into_string()
        .map_err(|error| Error::Http(error.to_string()))
}

/// GET JSON décodé.
pub fn get_json(url: &str, token: Option<&str>) -> Result<serde_json::Value> {
    let text = get(url, token)?;
    serde_json::from_str(&text).map_err(|error| Error::Parse(error.to_string()))
}

/// GET JSON sous l'identité d'un autre client.
pub fn get_json_as(url: &str, token: Option<&str>, agent: &str) -> Result<serde_json::Value> {
    let text = get_as(url, token, agent)?;
    serde_json::from_str(&text).map_err(|error| Error::Parse(error.to_string()))
}

/// POST de formulaire, avec authentification Basic facultative : c'est la
/// forme des jetons OAuth d'Epic.
pub fn post_form(
    url: &str,
    basic: Option<(&str, &str)>,
    form: &[(&str, &str)],
) -> Result<serde_json::Value> {
    let body: Vec<(&str, &str)> = form.to_vec();
    let mut request = ureq::post(url)
        .set("User-Agent", USER_AGENT)
        .set("Content-Type", "application/x-www-form-urlencoded")
        .timeout(std::time::Duration::from_secs(TIMEOUT));
    let header = basic.map(|(user, password)| {
        format!(
            "Basic {}",
            base64_encode(format!("{user}:{password}").as_bytes())
        )
    });
    if let Some(header) = &header {
        request = request.set("Authorization", header);
    }
    let response = request.send_form(&body)?;
    let text = response
        .into_string()
        .map_err(|error| Error::Http(error.to_string()))?;
    serde_json::from_str(&text).map_err(|error| Error::Parse(error.to_string()))
}

/// POST JSON authentifié : la forme des appels au compte (rattacher une offre
/// gratuite, par exemple).
pub fn post_json_bearer(
    url: &str,
    token: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value> {
    let response = ureq::post(url)
        .set("User-Agent", USER_AGENT)
        .set("Content-Type", "application/json")
        .set("Authorization", &format!("bearer {token}"))
        .timeout(std::time::Duration::from_secs(TIMEOUT))
        .send_string(&body.to_string());
    let text = match response {
        Ok(response) => response
            .into_string()
            .map_err(|error| Error::Http(error.to_string()))?,
        Err(ureq::Error::Status(code, response)) => {
            let text = response.into_string().unwrap_or_default();
            return Err(Error::Http(format!("Epic a refusé l'offre ({code}) : {}", short_reason(&text))));
        }
        Err(error) => return Err(Error::Http(error.to_string())),
    };
    if text.trim().is_empty() {
        return Ok(serde_json::Value::Null);
    }
    serde_json::from_str(&text).map_err(|error| Error::Parse(error.to_string()))
}

/// Message d'erreur de la boutique, sans le pavé JSON qui l'entoure.
fn short_reason(text: &str) -> String {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        for key in ["errorMessage", "message", "error_description", "errorCode"] {
            if let Some(message) = value.get(key).and_then(serde_json::Value::as_str) {
                if !message.trim().is_empty() {
                    return message.trim().to_string();
                }
            }
        }
    }
    text.chars().take(160).collect()
}

/// Texte simple : balises de la boutique retirées, entités courantes décodées
/// et lignes vides resserrées. Les descriptions Epic et GOG arrivent en HTML,
/// la fiche n'a pas de moteur de rendu web.
pub fn plain_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' => in_tag = false,
            _ if in_tag => {}
            _ => out.push(ch),
        }
    }
    let text = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&eacute;", "é")
        .replace("&egrave;", "è")
        .replace("&agrave;", "à")
        .replace("&ccedil;", "ç")
        .replace("&rsquo;", "’");
    let mut cleaned = String::with_capacity(text.len());
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if !cleaned.is_empty() {
            cleaned.push('\n');
        }
        cleaned.push_str(line);
    }
    cleaned
}

/// Jours depuis l'époque pour une date civile (algorithme de Howard
/// Hinnant), puis secondes Unix à midi UTC : une date de publication n'a pas
/// d'heure, midi évite de basculer d'un jour selon le fuseau.
pub fn civil_seconds(year: i64, month: i64, day: i64) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || !(1970..=9999).contains(&year) {
        return None;
    }
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + 43_200)
}

/// Date ISO 8601 (`2026-07-30T16:00:00.000Z`) → secondes Unix. Seule la date
/// est retenue : l'heure d'une annonce de boutique n'apprend rien.
pub fn iso_seconds(text: &str) -> Option<i64> {
    let text = text.trim();
    let mut parts = text.split(['-', 'T', ' ']);
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.get(..2)?.parse().ok()?;
    civil_seconds(year, month, day)
}

/// Base64 standard : l'en-tête Basic d'Epic.
pub fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let value = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(value >> 18) as usize & 63] as char);
        out.push(ALPHABET[(value >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(value >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[value as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Télécharge un fichier en flux, en rendant compte de l'avancement.
///
/// `progress` reçoit (octets reçus, total annoncé ou zéro) et rend `false`
/// quand la tâche doit s'arrêter : le fichier partiel est supprimé.
pub fn download(
    url: &str,
    token: Option<&str>,
    file: &Path,
    total_hint: u64,
    progress: &mut dyn FnMut(u64, u64) -> bool,
) -> Result<u64> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut request = ureq::get(url)
        .set("User-Agent", USER_AGENT)
        .timeout(std::time::Duration::from_secs(600));
    let bearer = token.map(|token| format!("Bearer {token}"));
    if let Some(bearer) = &bearer {
        request = request.set("Authorization", bearer);
    }
    let response = request.call()?;
    let announced = response
        .header("Content-Length")
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|size| *size > 0)
        .unwrap_or(total_hint);
    let part = file.with_extension("part");
    let mut out = std::fs::File::create(&part)?;
    let mut reader = response.into_reader();
    let mut buffer = vec![0u8; 128 * 1024];
    let mut done = 0u64;
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) => {
                let _ = std::fs::remove_file(&part);
                return Err(Error::Http(error.to_string()));
            }
        };
        out.write_all(&buffer[..read])?;
        done += read as u64;
        if !progress(done, announced) {
            let _ = std::fs::remove_file(&part);
            return Err(Error::Other("téléchargement annulé".into()));
        }
    }
    out.flush()?;
    std::fs::rename(&part, file)?;
    Ok(done)
}

/// Extrait une archive zip dans `dest`.
pub fn unzip(file: &Path, dest: &Path) -> Result<()> {
    let handle =
        std::fs::File::open(file).map_err(|error| Error::Archive(error.to_string()))?;
    let mut archive =
        zip::ZipArchive::new(handle).map_err(|error| Error::Archive(error.to_string()))?;
    std::fs::create_dir_all(dest)?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| Error::Archive(error.to_string()))?;
        let Some(path) = entry.enclosed_name() else {
            continue;
        };
        let target = dest.join(path);
        if entry.is_dir() {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::fs::File::create(&target)?;
        std::io::copy(&mut entry, &mut out)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Some(mode) = entry.unix_mode() {
                let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode));
            }
        }
    }
    Ok(())
}

/// Lance un installeur GOG Linux (mojosetup) sans question ni licence à
/// accepter une par une : l'utilisateur a déjà accepté en achetant le jeu, et
/// un installeur interactif bloquerait le daemon.
pub fn run_gog_installer(file: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    let status = std::process::Command::new("sh")
        .arg(file)
        .arg("--")
        .arg("--i-agree-to-all-licenses")
        .arg("--noreadme")
        .arg("--noprompt")
        .arg("--noexec")
        .arg("--keep")
        .arg("--destination")
        .arg(dest)
        .status()
        .map_err(|error| Error::Other(format!("installeur GOG : {error}")))?;
    if !status.success() {
        return Err(Error::Other(format!(
            "l'installeur GOG a échoué ({status})"
        )));
    }
    // `--noexec` garde l'arborescence extraite : on la déplace à la racine du
    // dossier d'installation, où le scan la trouve.
    hoist_single_directory(dest);
    Ok(())
}

/// Extrait un installeur Windows GOG avec `innoextract`.
///
/// L'outil est cherché dans le `PATH`, puis dans le dossier d'outils de
/// Lapinux, où il est téléchargé une fois s'il manque : sans lui, aucun
/// installeur Windows de GOG ne s'ouvre, et la plupart des jeux du magasin
/// n'existent que dans cette version.
pub fn run_innoextract(file: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    let bin = innoextract_path()?;
    let status = std::process::Command::new(&bin)
        .arg("--gog")
        .arg("-s")
        .arg("-d")
        .arg(dest)
        .arg(file)
        .status()
        .map_err(|error| {
            Error::Other(format!("innoextract n'a pas pu démarrer : {error}"))
        })?;
    if !status.success() {
        return Err(Error::Other("innoextract a échoué".into()));
    }
    hoist_single_directory(dest);
    Ok(())
}

/// Où trouver `innoextract` : le système d'abord, puis le binaire que Lapinux
/// installe à la demande dans son dossier d'outils.
fn innoextract_path() -> Result<PathBuf> {
    if let Some(path) = in_path("innoextract") {
        return Ok(path);
    }
    let tools = lapinux_core::paths::LapinuxPaths::discover().tools_dir;
    let local = tools.join("innoextract");
    if usable(&local) {
        return Ok(local);
    }
    install_innoextract(&tools)?;
    if usable(&local) {
        return Ok(local);
    }
    Err(Error::Other(
        "innoextract manque et n'a pas pu être installé".into(),
    ))
}

/// Vrai pour un binaire plausible : l'archive d'innoextract contient aussi un
/// script de lancement du même nom, inutilisable seul.
fn usable(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|meta| meta.len() > 64 * 1024)
        .unwrap_or(false)
}

/// Cherche un exécutable dans le `PATH`.
fn in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// Télécharge le binaire Linux publié par innoextract et le pose dans le
/// dossier d'outils : une archive `tar.xz`, dépaquetée par `tar`.
fn install_innoextract(tools: &Path) -> Result<()> {
    const URL: &str = "https://github.com/dscharrer/innoextract/releases/download/1.9/innoextract-1.9-linux.tar.xz";
    std::fs::create_dir_all(tools)?;
    let archive = tools.join("innoextract.tar.xz");
    download(URL, None, &archive, 0, &mut |_, _| true)?;
    let staging = tools.join("innoextract-unpack");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let status = std::process::Command::new("tar")
        .arg("-xJf")
        .arg(&archive)
        .arg("-C")
        .arg(&staging)
        .status()
        .map_err(|error| Error::Other(format!("tar : {error}")))?;
    let _ = std::fs::remove_file(&archive);
    if !status.success() {
        return Err(Error::Other("dépaquetage d'innoextract impossible".into()));
    }
    let Some(found) = find_binary(&staging, "innoextract") else {
        return Err(Error::Other(
            "l'archive d'innoextract ne contient pas son binaire".into(),
        ));
    };
    let target = tools.join("innoextract");
    let _ = std::fs::remove_file(&target);
    std::fs::copy(&found, &target)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))?;
    }
    let _ = std::fs::remove_dir_all(&staging);
    Ok(())
}

/// Plus gros fichier portant ce nom, sous quelque sous-dossier que ce soit :
/// l'archive range le vrai binaire à côté d'un script du même nom.
fn find_binary(root: &Path, name: &str) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    let mut best: Option<(u64, PathBuf)> = None;
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().and_then(|value| value.to_str()) == Some(name) {
                let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
                if best.as_ref().map(|(seen, _)| size > *seen).unwrap_or(true) {
                    best = Some((size, path));
                }
            }
        }
    }
    best.map(|(_, path)| path)
}

/// Si le dossier ne contient qu'un répertoire, son contenu remonte d'un cran :
/// les installeurs créent volontiers `dest/Jeu/…`.
pub fn hoist_single_directory(dest: &Path) {
    let Ok(entries) = std::fs::read_dir(dest) else {
        return;
    };
    let entries: Vec<_> = entries.flatten().collect();
    if entries.len() != 1 || !entries[0].path().is_dir() {
        return;
    }
    let inner = entries[0].path();
    let Ok(children) = std::fs::read_dir(&inner) else {
        return;
    };
    for child in children.flatten() {
        let target = dest.join(child.file_name());
        let _ = std::fs::rename(child.path(), target);
    }
    let _ = std::fs::remove_dir(&inner);
}

/// Nom de dossier sûr pour un jeu : ni barre oblique, ni caractère de contrôle.
pub fn install_dir(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').to_string();
    if trimmed.is_empty() {
        "Jeu".to_string()
    } else {
        trimmed.chars().take(80).collect()
    }
}

/// Écrit le manifeste Lapinux d'un jeu de boutique : un `.acf` que le scan
/// reconnaît, et un fichier d'identité dans le dossier du jeu qui dit de quelle
/// boutique il vient et sous quel identifiant.
pub fn write_store_manifest(
    library_root: &Path,
    appid: u32,
    name: &str,
    installdir: &str,
    store: Store,
    store_id: &str,
    size_on_disk: u64,
) -> Result<()> {
    let steamapps = library_root.join("steamapps");
    std::fs::create_dir_all(&steamapps)?;
    let common = steamapps.join("common").join(installdir);
    std::fs::create_dir_all(&common)?;
    let identity = serde_json::json!({
        "store": store.key(),
        "store_id": store_id,
        "name": name,
    });
    std::fs::write(
        common.join(".lapinux-store.json"),
        serde_json::to_string_pretty(&identity).unwrap_or_default(),
    )?;
    let escaped_name = name.replace('"', "'");
    let escaped_dir = installdir.replace('"', "'");
    let text = format!(
        "\"AppState\"\n{{\n\t\"appid\"\t\t\"{appid}\"\n\t\"name\"\t\t\"{escaped_name}\"\n\t\"StateFlags\"\t\t\"4\"\n\t\"installdir\"\t\t\"{escaped_dir}\"\n\t\"SizeOnDisk\"\t\t\"{size_on_disk}\"\n\t\"LastUpdated\"\t\t\"{}\"\n\t\"LapinuxManaged\"\t\t\"1\"\n}}\n",
        lapinux_core::util::now_unix()
    );
    std::fs::write(steamapps.join(format!("appmanifest_{appid}.acf")), text)?;
    Ok(())
}

/// Chemin publié par une boutique, ramené sous `root` : un nom absolu ou
/// remontant (`../`) est refusé plutôt que suivi. Un manifeste vient du réseau
/// et n'a pas à décider où l'on écrit.
pub fn join_rel(root: &Path, name: &str) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for component in Path::new(name).components() {
        match component {
            std::path::Component::Normal(part) => out.push(part),
            std::path::Component::CurDir => {}
            _ => {
                return Err(Error::Parse(format!(
                    "chemin refusé dans le manifeste : {name}"
                )))
            }
        }
    }
    if out.as_os_str().is_empty() {
        return Err(Error::Parse("chemin vide dans le manifeste".into()));
    }
    Ok(root.join(out))
}

/// Taille d'un dossier, fichiers compris.
pub fn directory_size(path: &Path) -> u64 {
    fn walk(path: &Path, total: &mut u64) {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                walk(&entry.path(), total);
            } else {
                *total += meta.len();
            }
        }
    }
    let mut total = 0;
    walk(path, &mut total);
    total
}

/// Extrait un paramètre d'une adresse collée (`code=…`, `authorizationCode`).
///
/// L'utilisateur colle souvent l'adresse entière de retour plutôt que le seul
/// code : on accepte les deux, et le JSON d'Epic.
pub fn extract_code(input: &str) -> Option<String> {
    let input = input.trim();
    for marker in ["code=", "authorizationCode\":", "code\":"] {
        if let Some(index) = input.find(marker) {
            let rest = &input[index + marker.len()..];
            // `{"authorizationCode": "abc"}` : l'espace et les guillemets
            // précèdent le code.
            let rest = rest.trim_start().trim_start_matches('"').trim_start();
            let end = rest
                .find(|c: char| c == '&' || c == '"' || c == '}' || c.is_whitespace())
                .unwrap_or(rest.len());
            let code = &rest[..end];
            if !code.is_empty() {
                return Some(code.to_string());
            }
        }
    }
    if input.is_empty() {
        None
    } else {
        Some(input.to_string())
    }
}

/// Dossier du cache des téléchargements de boutiques.
pub fn downloads_dir(data_dir: &Path, store: Store) -> PathBuf {
    data_dir.join("downloads").join(store.key())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_alphabet() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"client:secret"), "Y2xpZW50OnNlY3JldA==");
    }

    #[test]
    fn a_pasted_return_url_gives_its_code() {
        assert_eq!(
            extract_code("https://embed.gog.com/on_login_success?origin=client&code=ABC123"),
            Some("ABC123".into())
        );
        assert_eq!(
            extract_code(r#"{"authorizationCode": "epic-code"}"#),
            Some("epic-code".into())
        );
        assert_eq!(extract_code("  ep-code  "), Some("ep-code".into()));
        assert_eq!(extract_code("   "), None);
    }

    #[test]
    fn install_directories_have_no_path_separator() {
        assert_eq!(install_dir("Songs of Silence"), "Songs of Silence");
        assert_eq!(install_dir("A/B: C"), "A_B_ C");
        assert_eq!(install_dir("  ..  "), "Jeu");
    }

    #[test]
    fn a_single_extracted_directory_is_hoisted() {
        let root = std::env::temp_dir().join(format!(
            "lapinux-hoist-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Jeu/data")).unwrap();
        std::fs::write(root.join("Jeu/game.exe"), b"x").unwrap();
        hoist_single_directory(&root);
        assert!(root.join("game.exe").exists());
        assert!(!root.join("Jeu").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_zip_is_extracted_where_asked() {
        let root = std::env::temp_dir().join(format!(
            "lapinux-unzip-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let archive = root.join("game.zip");
        {
            let file = std::fs::File::create(&archive).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("bin/game", options).unwrap();
            zip.write_all(b"#!/bin/sh\n").unwrap();
            zip.finish().unwrap();
        }
        let dest = root.join("out");
        unzip(&archive, &dest).expect("extraction");
        assert!(dest.join("bin/game").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_manifest_names_the_store_and_its_game() {
        let root = std::env::temp_dir().join(format!(
            "lapinux-store-manifest-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        write_store_manifest(
            &root,
            0x8000_0001,
            "Jeu test",
            "Jeu test",
            Store::Gog,
            "1207658691",
            42,
        )
        .expect("manifeste");
        assert!(root
            .join("steamapps/appmanifest_2147483649.acf")
            .exists());
        assert!(root
            .join("steamapps/common/Jeu test/.lapinux-store.json")
            .exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_plan_sums_only_its_downloads() {
        let plan = InstallPlan {
            store: Store::Itch,
            store_id: "1".into(),
            name: "Jeu".into(),
            installdir: "Jeu".into(),
            size: 300,
            installed: 0,
            platform: "linux".into(),
            build: String::new(),
            steps: vec![
                Step::Download {
                    url: "https://example/a".into(),
                    file: "a".into(),
                    size: 100,
                },
                Step::Download {
                    url: "https://example/b".into(),
                    file: "b".into(),
                    size: 200,
                },
                Step::Unzip { file: "a".into() },
            ],
        };
        assert_eq!(plan.download_size(), 300);
    }
}
