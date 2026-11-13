//! Sonde temporaire : téléchargement d'une jaquette itch.io et conversion.
fn main() {
    let url = std::env::args().nth(1).expect("adresse");
    let response = ureq::get(&url).set("User-Agent", "Lapinux/0.1").call();
    match response {
        Ok(response) => {
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut response.into_reader(), &mut bytes).expect("lecture");
            println!("{} octets", bytes.len());
            match image::load_from_memory(&bytes) {
                Ok(image) => {
                    println!("image {} × {}", image.width(), image.height());
                    let width = image.width();
                    let height = ((width as f32) * 1.5).round() as u32;
                    let mut canvas =
                        image::RgbaImage::from_pixel(width, height, image::Rgba([20, 20, 20, 255]));
                    let top = height.saturating_sub(image.height()) / 2;
                    image::imageops::overlay(&mut canvas, &image.to_rgba8(), 0, i64::from(top));
                    let mut out = Vec::new();
                    image::DynamicImage::ImageRgba8(canvas)
                        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
                        .expect("encodage");
                    println!("converti en {} octets", out.len());
                }
                Err(error) => println!("décodage impossible : {error}"),
            }
        }
        Err(error) => println!("téléchargement impossible : {error}"),
    }
}
