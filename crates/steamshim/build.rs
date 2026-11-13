fn main() {
    // Le nom interne compte autant que le nom de fichier : un jeu lie
    // `libsteam_api.so` par son SONAME, et le préchargement ne prend sa place
    // que si les deux coïncident.
    println!("cargo:rustc-cdylib-link-arg=-Wl,-soname,libsteam_api.so");
}
