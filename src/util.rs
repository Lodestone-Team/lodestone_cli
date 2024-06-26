use crate::{info, versions::VersionWithV};
use color_eyre::{
    eyre::{Context, Result},
    owo_colors::OwoColorize,
};
use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};

use std::{
    env,
    path::{Path, PathBuf},
};
use tokio::io::AsyncWriteExt;

pub fn get_lodestone_path() -> Option<PathBuf> {
    let home_dir = dirs::home_dir()?;

    Some(match env::var("LODESTONE_PATH") {
        Ok(val) => PathBuf::from(val),
        Err(_) => home_dir.join(PathBuf::from(".lodestone")),
    })
}

pub fn executable_name_without_version() -> Option<String> {
    let target_arch = env::consts::ARCH;
    let target_os = env::consts::OS;
    match (target_arch, target_os) {
        ("x86_64", "windows") => Some("lodestone_core_windows_x86_64".to_string()),
        ("x86_64", "linux") => Some("lodestone_core_linux_x86_64".to_string()),
        ("aarch64", "linux") => Some("lodestone_core_linux_aarch".to_string()),
        ("aarch64", "macos") => Some("lodestone_core_macos_aarch".to_string()),
        _ => None,
    }
}

pub fn get_executable_name(version: &VersionWithV) -> String {
    // Get the target architecture and operating system
    let target_arch = env::consts::ARCH;
    let target_os = env::consts::OS;

    if let Some(executable_name) = executable_name_without_version() {
        if target_os == "windows" {
            format!("{}_{}.exe", executable_name, version)
        } else {
            format!("{}_{}", executable_name, version)
        }
    } else {
        panic!("Unsupported target architecture and operating system: {} {}", target_arch, target_os);
    }
}

pub async fn download_file(url: &str, dest: &Path, lodestone_path: &Path) -> Result<()> {
    info!("Downloading {} to {}", url, dest.display().bold().blue());
    let lodestone_tmp = lodestone_path.join("tmp");
    tokio::fs::create_dir_all(&lodestone_tmp)
        .await
        .context("Failed to create tmp dir")?;
    let temp_file_path = tempfile::NamedTempFile::new_in(lodestone_tmp)
        .context("Failed to create temporary file")?
        .path()
        .to_owned();
    let mut temp_file = tokio::fs::File::create(&temp_file_path)
        .await
        .context("Failed to create temporary file")?;
    let response = reqwest::get(url).await?;
    response.error_for_status_ref()?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let total = response.content_length().unwrap_or(0);
    let mut stream = response.bytes_stream();
    let pb = ProgressBar::new(total);
    pb.set_style(ProgressStyle::default_bar()
        .template("{msg}\n{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})").unwrap()
        .progress_chars("#>-"));
    pb.set_message(format!("Downloading {}", url));

    while let Some(item) = stream.next().await {
        let chunk = item?;
        temp_file.write_all(&chunk).await?;
        pb.inc(chunk.len() as u64);
    }
    pb.finish_with_message("Downloaded file");
    tokio::fs::rename(&temp_file_path, &dest)
        .await
        .context("Failed to move temporary file")?;

    Ok(())
}
