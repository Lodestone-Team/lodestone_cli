use chrono::Utc;
use color_eyre::{eyre::Result, owo_colors::OwoColorize};

use std::path::{Path, PathBuf};

pub mod download;
pub mod metadata;
use crate::{
    info, prompt_for_confirmation, update_manager::download::download_release, util, warn,
};

use crate::versions::{self, VersionWithV};

/// Updates the Lodestone Core to the latest release if needed
/// Returns the path to the new (or old) executable
pub async fn try_update(
    lodestone_path: &Path,
    version_override: Option<VersionWithV>,
    yes_all: bool,
    skip_update_check: bool,
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
                "We couldn't find an existing Lodestone Core installation under {}",
                lodestone_path.display(),
            );

            info!("If you would like to launch Lodestone Core in a different directory, rerun the cli with {}", "--install-path=<your path>".bold().blue());
            // if lodestone_path is not empty, exit
            if lodestone_path.read_dir()?.next().is_some() {
                warn!(
                    "{}, this is normal if you ran an older version of Lodestone Core",
                    format!(
                        "Path {} is not empty",
                        lodestone_path.display().bold().blue()
                    )
                );
                warn!("{} Doing so may break your installation of Lodestone Desktop", "If you have Lodestone Desktop, DO NOT INSTALL A DIFFERENT VERSION OF LODESTONE CORE.".bold().yellow());
                if !yes_all
                    && !prompt_for_confirmation(
                        format!(
                            "Would you like to install Lodestone Core {} to {}? {}:",
                            new_version.bold().blue(),
                            lodestone_path.display().bold().blue(),
                            "(yes/n)".magenta().bold()
                        ),
                        |s| s.trim() == "yes",
                    )
                {
                    info!("User chose not to install Lodestone Core, exiting");
                    return Ok(None);
                }
            }
            None
        }
    };
    match version_override {
        None => {
            if let Some(current_version) = current_version {
                if skip_update_check {
                    info!("Skipping update check, using current version");
                    return Ok(Some(
                        lodestone_path.join(util::get_executable_name(&current_version)?),
                    ));
                }
                info!(
                    "Current Lodestone Core version: {}, new Lodestone Core version: {}",
                    current_version.bold().blue(),
                    new_version.bold().blue(),
                );
                if current_version == new_version {
                    info!("Current Lodestone Core version is new version, skipping update");
                    return Ok(Some(
                        lodestone_path.join(util::get_executable_name(&current_version)?),
                    ));
                }

                if current_version > new_version {
                    info!("Current Lodestone Core version is greater than new version, skipping update");
                    return Ok(Some(
                        lodestone_path.join(util::get_executable_name(&current_version)?),
                    ));
                }

                // Otherwise we need to update
                // ask the user if they want to update in the terminal

                if !yes_all
                    && !prompt_for_confirmation(
                        format!(
                            "Would you like to update to {}? {}:",
                            new_version.bold().blue(),
                            "(y/n)".magenta().bold()
                        ),
                        |s| s.trim() == "y" || s.trim() == "yes",
                    )
                {
                    info!(
                        "You can skip update checks with the '{}' flag",
                        "--skip-update-check".bold().blue()
                    );
                    return Ok(Some(
                        lodestone_path.join(util::get_executable_name(&current_version)?),
                    ));
                }
            }
        }
        Some(v) => info!("Version override: {}", v.bold().yellow()),
    }

    let (executable_path, exe_file) = download_release(&new_version, lodestone_path).await?;

    let new_metadata = metadata::Metadata {
        current_version: new_version.clone(),
        last_updated: Utc::now().to_string(), //TODO Standardize this
        executable_name: exe_file,
    };

    new_metadata
        .write_metadata(&lodestone_path.join(".lodestone_cli_metadata.json"))
        .await?;

    info!(
        "{}",
        format!("Installed lodestone {new_version}").green().bold()
    );
    Ok(Some(executable_path))
}
