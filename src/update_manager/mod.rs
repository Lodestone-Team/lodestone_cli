use chrono::Utc;
use color_eyre::{eyre::Result, owo_colors::OwoColorize};
use semver::Version;
use std::path::{Path, PathBuf};

use tracing::{info, warn};

pub mod download;
pub mod metadata;
pub mod versions;
use crate::{prompt_for_confirmation, update_manager::download::download_release, util};

/// Updates the lodestone core to the latest release if needed
/// Returns the path to the new (or old) executable
pub async fn try_update(
    lodestone_path: &Path,
    version_override: Option<Version>,
    yes_all: bool,
) -> Result<Option<PathBuf>> {
    let new_version = if let Some(ref v) = version_override {
        v.clone()
    } else {
        versions::get_latest_release().await?
    };

    let current_version = match versions::get_current_version().await {
        Ok(v) => Some(v),
        Err(_e) => {
            info!(
                "We couldn't find a lodestone installation under {}",
                lodestone_path.display()
            );
            #[cfg(target_os = "windows")]
            {
                info!("If you have lodestone installed to a custom location, please shut down the launcher and follow the instructions at https://github.com/Lodestone-Team/lodestone/wiki/How-Tos#how-do-i-change-where-lodestone-stores-all-its-data");
            }
            #[cfg(target_os = "linux")]
            {
                info!("If you have lodestone installed to a custom location, please shut down the launcher and set the LODESTONE_PATH environment variable to the path to your lodestone core installation with `export LODESTONE_PATH=<path>`");
            }
            // if lodestone_path is not empty, exit
            if lodestone_path.read_dir()?.next().is_some() {
                info!("Path {} is not empty, this is normal if you ran an older version of lodestone core", lodestone_path.display());
                warn!("{} Doing so may break your installation of Lodestone Desktop", "If you have Lodestone Desktop, DO NOT INSTALL A DIFFERENT VERSION OF LODESTONE CORE.".bold().yellow());
                if !yes_all && !prompt_for_confirmation(format!("Would you like to install lodestone core {} to {}? Choosing 'n' will exit the launcher. (yes/n)", new_version.bold(), lodestone_path.display().bold()), |s| s.trim() == "yes") {
                    info!("User chose not to install lodestone core, exiting");
                    return Ok(None);
                }
            }
            None
        }
    };
    match version_override {
        None => {
            if let Some(current_version) = current_version {
                info!(
                    "Current version: {}, new version: {}",
                    current_version.bold(),
                    new_version.bold()
                );
                if current_version == new_version {
                    info!("Current version is new version, skipping update");
                    return Ok(Some(
                        lodestone_path.join(util::get_executable_name(&current_version)?),
                    ));
                }

                if current_version > new_version {
                    warn!("Current version is greater than new version, skipping update");
                    return Ok(Some(
                        lodestone_path.join(util::get_executable_name(&current_version)?),
                    ));
                }

                // Otherwise we need to update
                // ask the user if they want to update in the terminal

                if !yes_all
                    && !prompt_for_confirmation(
                        format!(
                            "Would you like to update from {} to {}? (y/n)",
                            current_version.bold(),
                            new_version.bold()
                        ),
                        |s| s.trim() == "y" || s.trim() == "yes",
                    )
                {
                    return Ok(Some(
                        lodestone_path.join(util::get_executable_name(&current_version)?),
                    ));
                }
            }
        }
        Some(v) => info!("Version override: {}", v),
    }

    let (executable_path, exe_file) = download_release(&new_version, lodestone_path).await?;

    let new_metadata = metadata::Metadata {
        current_version: new_version.clone(),
        last_updated: Utc::now().to_string(), //TODO Standardize this
        executable_name: exe_file,
    };

    new_metadata
        .write_metadata(&lodestone_path.join(".lodestone_launcher_metadata.json"))
        .await?;

    info!("Installed lodestone v{}", new_version);
    Ok(Some(executable_path))
}
