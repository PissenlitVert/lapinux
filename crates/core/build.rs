//! Date de fabrication du binaire, affichée à côté de la version.
//!
//! `SOURCE_DATE_EPOCH` est respecté quand il est posé : une reconstruction
//! reproductible garde la même date.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    let date = from_source_date_epoch()
        .or_else(today)
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=LAPINUX_BUILD_DATE={date}");
}

fn from_source_date_epoch() -> Option<String> {
    let epoch = std::env::var("SOURCE_DATE_EPOCH").ok()?;
    let stamp = format!("@{epoch}");
    run_date(&["-u", "-d", &stamp, "+%Y-%m-%d"])
}

fn today() -> Option<String> {
    run_date(&["-u", "+%Y-%m-%d"])
}

fn run_date(arguments: &[&str]) -> Option<String> {
    let output = Command::new("date").args(arguments).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let date = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!date.is_empty()).then_some(date)
}
