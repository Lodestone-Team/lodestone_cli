use color_eyre::eyre::{eyre, Result};
use semver::Version;
use std::{env, path::PathBuf};
use tokio::fs;
use tracing::debug;

pub fn get_lodestone_path() -> PathBuf {
    let home_dir = dirs::home_dir().unwrap();

    match env::var("LODESTONE_PATH") {
        Ok(val) => PathBuf::from(val),
        Err(_) => home_dir.join(PathBuf::from(".lodestone")),
    }
}

pub fn get_metadata_path() -> PathBuf {
    let lodestone_path = get_lodestone_path();
    lodestone_path.join(PathBuf::from("metadata.json"))
    // return PathBuf::from("metadata.json");
}

pub fn get_executable_name(version: &Version) -> Result<String> {
    // Get the target architecture and operating system
    let target_arch = env::consts::ARCH;
    let target_os = env::consts::OS;

    let executable_name: String = match (target_arch, target_os) {
        ("x86_64", "windows") => format!("lodestone_core_windows_v{}.exe", version),
        ("aarch64", "linux") => format!("lodestone_core_arm_v{}", version),
        ("x86_64", "linux") => format!("lodestone_core_v{}", version),
        _ => {
            return Err(eyre!(
                "Unsupported target system {}-{}",
                target_os,
                target_arch
            ))
        }
    };

    Ok(executable_name)
}

pub async fn download_file(url: &str, file_name: &str) -> Result<PathBuf> {
    let response = reqwest::get(url).await?;
    let bytes = response.bytes().await?;

    let lodestone_path = get_lodestone_path();
    std::fs::create_dir_all(&lodestone_path)?;

    let executable_path = lodestone_path.join(file_name);
    fs::write(&executable_path, &bytes).await?;
    debug!("File written at path: {:?}", &executable_path);

    Ok(executable_path)
}
