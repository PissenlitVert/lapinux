use std::env;
use std::fs;
use std::path::PathBuf;

/// Taille maximale d'une vtable d'interface. `ISteamUGC021` en compte 96
/// (slots 0 à 95) ;
/// les quatre emplacements supplémentaires laissent de la marge aux versions
/// voisines sans jamais lire hors de la table.
const SLOTS: usize = 104;

/// Interfaces exposées par le pont, avec le nom qu'elles portent au journal.
/// Un « slot 41 » seul ne dit pas qui l'appelle ; « SteamFriends slot 41 » si.
const INTERFACES: [(&str, &str); 12] = [
    ("user", "SteamUser"),
    ("friends", "SteamFriends"),
    ("utils", "SteamUtils"),
    ("stats", "SteamUserStats"),
    ("matchmaking", "SteamMatchmaking"),
    ("apps", "SteamApps"),
    ("remote", "SteamRemoteStorage"),
    ("ugc", "SteamUGC"),
    ("input", "SteamInput"),
    ("net", "SteamNetworking"),
    ("servers", "SteamMatchmakingServers"),
    ("generic", "interface générique"),
];

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let mut src = String::from("// généré par build.rs : une table journalisée par interface\n");

    for (index, (tag, _)) in INTERFACES.iter().enumerate() {
        for slot in 0..SLOTS {
            src.push_str(&format!(
                "extern \"system\" fn debug_stub_{tag}_{slot}() -> u64 {{ crate::debug_slot({index}, {slot}); 0 }}\n"
            ));
        }
        src.push_str(&format!("fn stubs_{tag}() -> [usize; {SLOTS}] {{\n    [\n"));
        for slot in 0..SLOTS {
            src.push_str(&format!("        debug_stub_{tag}_{slot} as usize,\n"));
        }
        src.push_str("    ]\n}\n");
    }

    src.push_str(&format!(
        "const INTERFACE_LABELS: [&str; {}] = [\n",
        INTERFACES.len()
    ));
    for (_, label) in INTERFACES.iter() {
        src.push_str(&format!("    \"{label}\",\n"));
    }
    src.push_str("];\n");

    fs::write(out.join("generic_stubs.rs"), src).expect("écriture des stubs génériques");
    println!("cargo:rerun-if-changed=build.rs");
}
