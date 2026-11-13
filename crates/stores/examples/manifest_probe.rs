//! Sonde temporaire : manifeste Epic, chemin du premier chunk, et réponse du
//! CDN avec ou sans le jeton de l'adresse.
use std::io::Read;

fn main() {
    let mut args = std::env::args().skip(1);
    let manifest_url = args.next().expect("adresse du manifeste");
    let bytes = reqwest_get(&manifest_url).expect("manifeste");
    std::fs::write("/tmp/probe.manifest", &bytes).ok();
    let manifest = lapinux_stores::epic_manifest::parse(&bytes).expect("analyse");
    println!(
        "version {} · feature {} · {} fichiers · {} chunks · {} octets",
        manifest.build_version,
        manifest.feature_level,
        manifest.files.len(),
        manifest.chunks.len(),
        manifest.download_size()
    );
    let chunk = manifest.chunks.iter().find(|c| c.download_size() > 0);
    let Some(chunk) = chunk else {
        println!("aucun chunk avec données");
        return;
    };
    let path = manifest.chunk_path(chunk);
    println!(
        "chunk {} · {} · {} octets",
        path,
        manifest.chunk_dir(),
        chunk.download_size()
    );
    let clean = manifest_url.split('?').next().unwrap_or(&manifest_url);
    let base = &clean[..clean.rfind('/').map(|i| i + 1).unwrap_or(0)];
    let token = manifest_url
        .split_once('?')
        .map(|(_, q)| q.to_string())
        .unwrap_or_default();
    for (label, url) in [
        (
            "sans jeton",
            format!("{base}{}/{path}", manifest.chunk_dir()),
        ),
        (
            "avec jeton",
            format!("{base}{}/{path}?{token}", manifest.chunk_dir()),
        ),
    ] {
        match status(&url) {
            Ok(code) => println!("{label} → {code}"),
            Err(error) => println!("{label} → erreur {error}"),
        }
    }
}

fn status(url: &str) -> Result<u16, String> {
    match ureq::get(url)
        .set("User-Agent", lapinux_stores::common::USER_AGENT)
        .call()
    {
        Ok(response) => Ok(response.status()),
        Err(ureq::Error::Status(code, _)) => Ok(code),
        Err(error) => Err(error.to_string()),
    }
}

fn reqwest_get(url: &str) -> Result<Vec<u8>, String> {
    let response = ureq::get(url)
        .set("User-Agent", lapinux_stores::common::USER_AGENT)
        .call()
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut out)
        .map_err(|e| e.to_string())?;
    Ok(out)
}
