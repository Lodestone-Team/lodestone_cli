use super::manage_backup::{copy_dir, load_backup};
use dirs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

pub fn get_path() -> PathBuf {
    let home_dir = dirs::home_dir().unwrap();
    println!("Home directory: {:?}", home_dir);
    let lodestone_path = match env::var("LODESTONE_PATH") {
        Ok(val) => PathBuf::from(val),
        Err(_) => home_dir.join(PathBuf::from(".lodestone")),
    };
    println!("Lodestone path: {:?}", lodestone_path);
    return lodestone_path;
}

fn download_asset(asset_url: &str, exe_name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let response = reqwest::blocking::get(asset_url)?;
    let bytes = response.bytes()?;

    let lodestone_dir = get_path();
    std::fs::create_dir_all(&lodestone_dir)?;

    let exe_path = lodestone_dir.join(&exe_name);
    println!("Exe path: {:?}", exe_path);

    fs::write(&exe_path, &bytes)?;
    println!("file written");

    Ok(exe_path)
}

fn get_release_url_and_exe_file(
    version: &str,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    // Get the target architecture and operating system
    let target_arch = env::consts::ARCH;
    let target_os = env::consts::OS;

    // Choose the appropriate asset filename based on the target architecture and operating system
    let asset_url = match (target_arch, target_os) {
    ("x86_64", "windows") => format!(
        "https://github.com/Lodestone-Team/lodestone_core/releases/download/{}/lodestone_core_windows_{}.exe", version, version),
    ("arm", "linux") => format!(
        "https://github.com/Lodestone-Team/lodestone_core/releases/download/{}/lodestone_core_arm_{}.exe", version, version),
    ("x86", "linux") => format!(
        "https://github.com/Lodestone-Team/lodestone_core/releases/download/{}/lodestone_core_{}.exe", version, version),
    _ => return Err("Unsupported target system".into()),
}   ;

    let exe_file: String = match (target_arch, target_os) {
        ("x86_64", "windows") => format!("lodestone_core_windows_{}.exe", version),
        ("arm", "linux") => format!("lodestone_core_arm_{}.exe", version),
        ("x86", "linux") => format!("lodestone_core_{}.exe", version),
        _ => return Err("Unsupported target system".into()),
    };

    Ok((asset_url, exe_file))
}

pub fn download_release(version: &str) -> Result<(PathBuf, String), Box<dyn std::error::Error>> {
    let lodestone_dir = get_path();
    let dest_dir = lodestone_dir.join(PathBuf::from(".core_backup"));
    copy_dir(&lodestone_dir, &dest_dir).unwrap_or_else(|e| {
        eprintln!("Failed to copy directory: {}", e);
        std::process::exit(1);
    });

    let (asset_url, exe_file) = get_release_url_and_exe_file(&version)?;
    let exe_path = download_asset(&asset_url, &exe_file)?;

    Ok((exe_path, exe_file))
}
