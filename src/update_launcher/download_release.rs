use super::manage_backup::copy_dir;
use color_eyre::eyre::{eyre, Result};
use dirs;
use std::path::PathBuf;
use std::{env, fs};
use tracing::info;

pub fn get_lodestone_path() -> PathBuf {
    let home_dir = dirs::home_dir().unwrap();
    info!("Home directory: {:?}", home_dir);
    let lodestone_path = match env::var("LODESTONE_PATH") {
        Ok(val) => PathBuf::from(val),
        Err(_) => home_dir.join(PathBuf::from(".lodestone")),
    };
    info!("Lodestone path: {:?}", lodestone_path);
    return lodestone_path;
}

fn download_asset(asset_url: &str, exe_name: &str) -> Result<PathBuf> {
    let response = reqwest::blocking::get(asset_url)?;
    let bytes = response.bytes()?;

    let lodestone_path = get_lodestone_path();
    std::fs::create_dir_all(&lodestone_path)?;

    let exe_path = lodestone_path.join(&exe_name);
    fs::write(&exe_path, &bytes)?;
    info!("File written at path: {:?}", &exe_path);

    Ok(exe_path)
}

fn get_release_url_and_executable_name(version: &str) -> Result<(String, String)> {
    // Get the target architecture and operating system
    let target_arch = env::consts::ARCH;
    let target_os = env::consts::OS;
    let github_repo_url = "https://github.com/Lodestone-Team/lodestone_core/";

    let executable_name: String = match (target_arch, target_os) {
        ("x86_64", "windows") => format!("lodestone_core_windows_{}.exe", version),
        ("aarch64", "linux") => format!("lodestone_core_arm_{}", version),
        ("x86", "linux") => format!("lodestone_core_{}", version),
        _ => return Err(eyre!("Unsupported target system")),
    };

    // Choose the appropriate asset filename based on the target architecture and operating system
    let asset_url = match (target_arch, target_os) {
        ("x86_64", "windows") => format!(
            "{}releases/download/{}/{}",
            github_repo_url, version, executable_name
        ),
        ("aarch64", "linux") => format!(
            "{}releases/download/{}/{}",
            github_repo_url, version, executable_name
        ),
        ("x86", "linux") => format!(
            "{}releases/download/{}/{}",
            github_repo_url, version, executable_name
        ),
        _ => return Err(eyre!("Unsupported target system")),
    };

    Ok((asset_url, executable_name))
}

pub fn download_release(version: &str) -> Result<(PathBuf, String)> {
    // we try to backup the current core before downloading the new one
    let lodestone_path = get_lodestone_path();
    let dest_dir = lodestone_path.join(PathBuf::from(".core_backup"));
    copy_dir(&lodestone_path, &dest_dir)?;

    let (asset_url, executable_name) = get_release_url_and_executable_name(&version)?;
    let exe_path = download_asset(&asset_url, &executable_name)?;

    Ok((exe_path, executable_name))
}
