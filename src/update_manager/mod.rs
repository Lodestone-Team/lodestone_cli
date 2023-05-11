use chrono::Utc;
use color_eyre::eyre::Result;
use std::path::PathBuf;

use tracing::{info, warn};

pub mod download;
pub mod metadata;
pub mod versions;
use crate::{update_manager::download::download_release, util};


/// Updates the lodestone core to the latest release if needed
/// Returns the path to the new (or old) executable
pub async fn try_update() -> Result<PathBuf> {
    let latest_version = versions::get_latest_release().await?;
    let current_version = match versions::get_current_version().await {
        Ok(v) => Some(v),
        Err(_e) => {
            let lodestone_path = util::get_lodestone_path();
            info!(
                "We couldn't find a lodestone installation under {}",
                lodestone_path.display()
            );
            #[cfg(target_os = "windows")]
            {
                info!("If you have lodestone installed to a custom location, please restart the updater and set the LODESTONE_PATH environment variable to the path to your lodestone installation with `set LODESTONE_PATH=<path>`");
            }
            #[cfg(target_os = "linux")]
            {
                info!("If you have lodestone installed to a custom location, please restart the updater and set the LODESTONE_PATH environment variable to the path to your lodestone installation with `export LODESTONE_PATH=<path>`");
            }
            info!(
                "Would you like to install lodestone v{latest_version} to {}? Choosing 'n' will exit the updater. (y/n)",
                lodestone_path.display()
            );
            let mut answer = String::new();
            std::io::stdin().read_line(&mut answer)?;
            if answer.trim() != "y" {
                info!("User chose not to install");
                std::process::exit(0);
            } else {
                info!("User chose to install");
                None
            }
        }
    };
    let lodestone_path = util::get_lodestone_path();

    let current_version = match current_version {
        Some(current_version) => {
            info!(
                "Current version: v{}, Latest version: v{}",
                current_version, latest_version
            );
            if current_version == latest_version {
                info!("Current version is latest version, skipping update");
                return Ok(lodestone_path.join(util::get_executable_name(&current_version)?));
            }

            if current_version > latest_version {
                warn!("Current version is greater than latest release, skipping update");
                return Ok(lodestone_path.join(util::get_executable_name(&current_version)?));
            }

            // Otherwise we need to update
            // ask the user if they want to update in the terminal
            let mut answer = String::new();
            info!("Would you like to update to the latest version? (y/n)");
            std::io::stdin().read_line(&mut answer)?;
            if answer.trim() != "y" {
                info!("User chose not to update");
                return Ok(lodestone_path.join(util::get_executable_name(&current_version)?));
            }
            current_version
        }
        None => latest_version.clone(),
    };

    let (executable_path, exe_file) = download_release(&latest_version).await?;

    let new_metadata = metadata::Metadata {
        current_version: format!("v{}", latest_version),
        last_updated: Utc::now().to_string(), //TODO Standardize this
        executable_name: exe_file,
    };

    let metadata_path = crate::util::get_metadata_path();
    new_metadata
        .write_metadata(&lodestone_path.join(metadata_path))
        .await?;

    info!("Updated from {} to {}", current_version, latest_version);
    Ok(executable_path)
}
