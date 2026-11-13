//! Capture du jeu par KWin, sans portail ni outil externe.
//!
//! `org.kde.KWin.ScreenShot2` rend les dimensions dans sa réponse D-Bus
//! (`width`, `height`, `stride`, `format`) et écrit **les pixels bruts** dans
//! un tube passé en argument. Rien n'est demandé à l'utilisateur : KWin
//! compose l'image lui-même, aucun sélecteur de fenêtre ne s'ouvre.
//!
//! `CaptureWindow` prend la fenêtre du jeu elle-même : son contenu seul, jamais
//! ce qui se trouve dessous. `CaptureArea` reste le repli quand KWin ne donne
//! pas d'identifiant de fenêtre.
//!
//! KWin n'accorde cette interface qu'aux applications dont un fichier
//! `.desktop` déclare `X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2`
//! (voir `captures::ensure_permission_file`) ; il lit ces fichiers à son
//! démarrage.

use std::collections::HashMap;
use std::io::Read;
use std::os::fd::{FromRawFd, OwnedFd};

use zbus::blocking::Connection;
use zbus::zvariant::{Fd, Value};

const SERVICE: &str = "org.kde.KWin";
const PATH: &str = "/org/kde/KWin/ScreenShot2";
const INTERFACE: &str = "org.kde.KWin.ScreenShot2";

/// Une image capturée : pixels `BGRA` et leur pas de ligne réel, tel que KWin
/// les a écrits.
#[derive(Debug, Clone)]
pub struct Shot {
    pub width: u32,
    pub height: u32,
    /// Octets par ligne, alignés par KWin : souvent plus que `width * 4`.
    pub stride: u32,
    /// Format `QImage` : 4 = RGB32, 5 = ARGB32, 6 = ARGB32 prémultiplié,
    /// 16 = RGBX8888, 17 = RGBA8888, 18 = RGBA8888 prémultiplié. KWin rend
    /// souvent du RGBA sur un bureau Wayland récent ; les trois derniers
    /// inversent les canaux rouge et bleu par rapport à `BGRA`.
    pub format: u32,
    pub data: Vec<u8>,
}

impl Shot {
    /// Pixels rangés sans remplissage, à la taille exacte que `ffmpeg`
    /// attend : une image courte est complétée par du noir plutôt que de
    /// casser le flux vidéo.
    pub fn tight(&self) -> Vec<u8> {
        let row = (self.width * 4) as usize;
        let stride = self.stride as usize;
        let mut out = vec![0u8; row * self.height as usize];
        for line in 0..self.height as usize {
            let start = line * stride;
            let Some(chunk) = self.data.get(start..(start + row).min(self.data.len())) else {
                break;
            };
            out[line * row..line * row + chunk.len()].copy_from_slice(chunk);
        }
        out
    }

    /// Comme `tight`, mais rend le tampon de KWin tel quel quand ses lignes
    /// sont déjà compactes. C'est le cas courant : cela évite une copie de
    /// 8 Mo par image 1080p, soit 240 Mo/s à 30 i/s.
    pub fn into_tight(self) -> Vec<u8> {
        let row = (self.width * 4) as usize;
        if self.stride as usize == row && self.data.len() == row * self.height as usize {
            self.data
        } else {
            self.tight()
        }
    }

    /// Pixels compactés en `BGRA`, le format attendu par `ffmpeg`.
    ///
    /// KWin décrit son format `QImage` dans la réponse : 4, 5 et 6 sont déjà
    /// du `BGRA`, tandis que 16, 17 et 18 sont du `RGB(A)X8888`, dont les
    /// canaux rouge et bleu sont inversés. Les convertir ici évite d'écrire
    /// une vidéo aux couleurs fausses — ou de perdre l'enregistrement, comme
    /// lorsque le format 18 était refusé.
    pub fn into_bgra(self) -> Vec<u8> {
        let format = self.format;
        let mut data = self.into_tight();
        if matches!(format, 16 | 17 | 18) {
            for pixel in data.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
        }
        data
    }
}

/// KWin a-t-il rendu un format que [`Shot::into_bgra`] sait convertir ?
pub fn supported_format(format: u32) -> bool {
    matches!(format, 4 | 5 | 6 | 16 | 17 | 18)
}

/// Prépare le tube de pixels et le fil qui le lit : KWin y écrit plusieurs
/// mégaoctets, et un tube plein sans lecteur bloquerait l'appel avant sa
/// réponse.
fn pipe_reader() -> Result<(std::thread::JoinHandle<Vec<u8>>, OwnedFd), String> {
    let mut fds = [0i32; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err("tube de capture impossible".into());
    }
    // Un tube de 64 Kio demandait environ 128 réveils par image 1080p.
    // Un mégaoctet est le maximum non privilégié ici et divise ce nombre par
    // seize ; le lecteur draine toujours le tube en parallèle de KWin.
    let _ = unsafe { libc::fcntl(fds[0], libc::F_SETPIPE_SZ, 1_048_576) };
    let read = unsafe { std::fs::File::from_raw_fd(fds[0]) };
    let write = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    let reader = std::thread::spawn(move || {
        let mut file = read;
        // 1080p BGRA fait 8,3 Mio : réserver évite les réallocations
        // successives de `read_to_end`, qui recopiaient le tampon en cours
        // de lecture à chaque agrandissement.
        let mut bytes = Vec::with_capacity(16 * 1024 * 1024);
        let _ = file.read_to_end(&mut bytes);
        bytes
    });
    Ok((reader, write))
}

/// Lit la réponse de KWin : `ffmpeg` ne doit pas recevoir une image sans sa
/// taille, et la réponse donne les quatre valeurs qui décrivent le flux.
fn collect(reply: zbus::Message, reader: std::thread::JoinHandle<Vec<u8>>) -> Result<Shot, String> {
    let body = reply.body();
    let meta: HashMap<String, Value> = body
        .deserialize()
        .map_err(|err| format!("réponse de capture illisible : {err}"))?;
    let width = meta_u32(&meta, "width").ok_or("capture sans largeur")?;
    let height = meta_u32(&meta, "height").ok_or("capture sans hauteur")?;
    let stride = meta_u32(&meta, "stride").ok_or("capture sans pas de ligne")?;
    let format = meta_u32(&meta, "format").unwrap_or(4);
    let data = reader
        .join()
        .map_err(|_| "lecture de la capture interrompue".to_string())?;
    let needed = stride as usize * height as usize;
    if data.len() < needed {
        return Err(format!(
            "capture incomplète : {} octets pour {needed} attendus",
            data.len()
        ));
    }
    Ok(Shot {
        width,
        height,
        stride,
        format,
        data,
    })
}

/// Connexion D-Bus gardée pendant toute la capture.
///
/// Une capture par image ouvrait auparavant une connexion au bus de session :
/// à 20 i/s, cela faisait vingt poignées de main par seconde sur le chemin
/// critique du compositeur. La connexion ne dépend pas de l'image ; elle peut
/// vivre aussi longtemps que la boucle de capture.
pub struct Capturer {
    connection: Connection,
}

impl Capturer {
    pub fn new() -> Result<Self, String> {
        let connection = Connection::session().map_err(|err| format!("bus de session : {err}"))?;
        Ok(Self { connection })
    }

    /// Capture une zone du bureau, en coordonnées du bureau — celles que KWin
    /// donne pour une fenêtre.
    pub fn capture_area(&self, x: i32, y: i32, width: u32, height: u32) -> Result<Shot, String> {
        let (reader, write) = pipe_reader()?;
        let options: HashMap<String, Value> = HashMap::new();
        let reply = self
            .connection
            .call_method(
                Some(SERVICE),
                PATH,
                Some(INTERFACE),
                "CaptureArea",
                &(x, y, width, height, &options, Fd::from(write)),
            )
            .map_err(|err| format!("capture KWin : {err}"))?;
        collect(reply, reader)
    }

    /// Capture une **fenêtre** par l'identifiant que KWin lui donne
    /// (`internalId`, celui que `CaptureWindow` attend).
    ///
    /// C'est ce qui n'enregistre que le jeu : une zone de l'écran montre le
    /// bureau avant la première image du jeu et après sa fermeture, tandis
    /// qu'une fenêtre ne porte que son propre contenu — et disparaît avec elle.
    pub fn capture_window(&self, handle: &str) -> Result<Shot, String> {
        let (reader, write) = pipe_reader()?;
        // La fenêtre du jeu n'a ni ombre ni décoration à conserver. Les
        // demander quand même agrandit la texture rendue et la lecture GPU,
        // pour des pixels que la vidéo n'utilise pas. `native-resolution`
        // reste faux : la cible 1080p est déjà fixée par les réglages.
        let mut options: HashMap<String, Value> = HashMap::new();
        options.insert("include-shadow".into(), Value::Bool(false));
        options.insert("include-decoration".into(), Value::Bool(false));
        options.insert("include-cursor".into(), Value::Bool(false));
        options.insert("native-resolution".into(), Value::Bool(false));
        let reply = self
            .connection
            .call_method(
                Some(SERVICE),
                PATH,
                Some(INTERFACE),
                "CaptureWindow",
                &(handle, &options, Fd::from(write)),
            )
            .map_err(|err| format!("capture KWin : {err}"))?;
        collect(reply, reader)
    }
}

/// Capture une zone avec une connexion neuve. Pour une boucle, préférez
/// [`Capturer`] : la connexion est alors réutilisée d'une image à l'autre.
pub fn capture_area(x: i32, y: i32, width: u32, height: u32) -> Result<Shot, String> {
    Capturer::new()?.capture_area(x, y, width, height)
}

/// Capture une fenêtre avec une connexion neuve. Voir [`Capturer`] pour une
/// capture répétée.
pub fn capture_window(handle: &str) -> Result<Shot, String> {
    Capturer::new()?.capture_window(handle)
}

/// Une valeur nommée de la réponse, en entier non signé.
fn meta_u32(meta: &HashMap<String, Value>, key: &str) -> Option<u32> {
    match meta.get(key)? {
        Value::U32(value) => Some(*value),
        Value::I32(value) => Some(*value as u32),
        Value::U64(value) => u32::try_from(*value).ok(),
        Value::I64(value) => u32::try_from(*value).ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_pixels_sont_rangees_sans_remplissage() {
        let shot = Shot {
            width: 2,
            height: 2,
            stride: 12,
            format: 4,
            data: vec![
                1, 2, 3, 4, 5, 6, 7, 8, 9, 9, 9, 9, // ligne 1 + remplissage
                10, 20, 30, 40, 50, 60, 70, 80, 9, 9, 9, 9,
            ],
        };
        let tight = shot.tight();
        assert_eq!(tight.len(), 16);
        assert_eq!(&tight[0..8], &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(&tight[8..16], &[10, 20, 30, 40, 50, 60, 70, 80]);
    }

    #[test]
    fn une_image_compacte_est_rendue_sans_copie() {
        let data = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let shot = Shot {
            width: 2,
            height: 1,
            stride: 8,
            format: 4,
            data: data.clone(),
        };
        assert_eq!(shot.into_tight(), data);
    }

    /// Une capture tronquée garde la taille d'une image entière : `ffmpeg`
    /// refuse un flux dont les images changent de taille.
    #[test]
    fn une_capture_courte_est_completee_en_noir() {
        let shot = Shot {
            width: 2,
            height: 2,
            stride: 8,
            format: 4,
            data: vec![1, 2, 3, 4, 5, 6, 7, 8],
        };
        assert_eq!(shot.tight().len(), 16);
        assert_eq!(&shot.tight()[8..16], &[0; 8]);
    }

    #[test]
    fn le_rgba_de_kwin_est_converti_en_bgra() {
        // Format 18 : QImage::Format_RGBA8888_Premultiplied, rendu par KWin
        // sur un bureau Wayland récent. Sans conversion, le rouge et le bleu
        // étaient inversés et la capture s'arrêtait.
        let shot = Shot {
            width: 2,
            height: 1,
            stride: 8,
            format: 18,
            data: vec![10, 20, 30, 255, 40, 50, 60, 255],
        };
        assert_eq!(shot.into_bgra(), vec![30, 20, 10, 255, 60, 50, 40, 255]);
    }

    #[test]
    fn le_rgbx_de_kwin_est_converti_en_bgra() {
        // Format 16 : QImage::Format_RGBX8888, celui que renvoie KWin sur ce
        // bureau. Le quatrième octet vaut 255 (canal ignoré).
        let shot = Shot {
            width: 2,
            height: 1,
            stride: 8,
            format: 16,
            data: vec![10, 20, 30, 255, 40, 50, 60, 255],
        };
        assert_eq!(shot.into_bgra(), vec![30, 20, 10, 255, 60, 50, 40, 255]);
    }

    #[test]
    fn les_metadonnees_se_lisent_en_entier() {
        let mut meta: HashMap<String, Value> = HashMap::new();
        meta.insert("width".into(), Value::U32(1920));
        meta.insert("height".into(), Value::I32(1080));
        assert_eq!(meta_u32(&meta, "width"), Some(1920));
        assert_eq!(meta_u32(&meta, "height"), Some(1080));
        assert_eq!(meta_u32(&meta, "stride"), None);
    }

    /// Compare une connexion neuve par image à une connexion réutilisée.
    /// À lancer à la main sur un bureau Wayland :
    /// `cargo test -p lapinux-daemon kwin_shot -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn la_connexion_reutilisee_reduit_la_latence() {
        use std::time::{Duration, Instant};

        let mut fresh = Vec::new();
        for _ in 0..30 {
            let start = Instant::now();
            capture_area(0, 0, 320, 200).expect("capture KWin");
            fresh.push(start.elapsed());
        }
        let reused = Capturer::new().expect("bus de session");
        let mut shared = Vec::new();
        for _ in 0..30 {
            let start = Instant::now();
            reused
                .capture_area(0, 0, 320, 200)
                .expect("capture KWin réutilisée");
            shared.push(start.elapsed());
        }
        fresh.sort();
        shared.sort();
        let median = |values: &[Duration]| values[values.len() / 2];
        println!(
            "capture 320x200 · connexion neuve : {:?} · connexion réutilisée : {:?}",
            median(&fresh),
            median(&shared)
        );
    }

    /// Capture réelle, à lancer à la main : `cargo test -p lapinux-daemon
    /// kwin_shot -- --ignored --nocapture`. Elle prouve l'appel KWin et la
    /// forme de la réponse sur la machine.
    #[test]
    #[ignore]
    fn capture_reelle_d_une_zone() {
        let shot = capture_area(0, 0, 320, 200).expect("capture KWin");
        println!(
            "image {}x{} · pas {} · format {} · {} octets",
            shot.width,
            shot.height,
            shot.stride,
            shot.format,
            shot.data.len()
        );
    }
}
