use color_eyre::eyre::{eyre, Result};
use semver::Version;
use std::{
    env,
    path::{Path, PathBuf},
};
use tokio::fs;
use tracing::debug;

pub fn get_lodestone_path() -> Option<PathBuf> {
    let home_dir = dirs::home_dir()?;

    Some(match env::var("LODESTONE_PATH") {
        Ok(val) => PathBuf::from(val),
        Err(_) => home_dir.join(PathBuf::from(".lodestone")),
    })
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

pub async fn download_file(url: &str, dest: &Path) -> Result<()> {
    let response = reqwest::get(url).await?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = response.bytes().await?;

    fs::write(&dest, &bytes).await?;
    debug!("Downloaded file to path {}", dest.display());

    Ok(())
}
