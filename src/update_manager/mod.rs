use chrono::Utc;
use color_eyre::{eyre::Result, owo_colors::OwoColorize};
use semver::Version;
use std::path::{Path, PathBuf};



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
            println!(
                "We couldn't find an existing lodestone core installation under {}",
                lodestone_path.display()
            );
            #[cfg(target_os = "windows")]
            {
                println!("If you have lodestone installed to a custom location, please shut down the launcher and follow the instructions at {}", "https://github.com/Lodestone-Team/lodestone/wiki/How-Tos#how-do-i-change-where-lodestone-stores-all-its-data".underline());
            }
            #[cfg(target_os = "linux")]
            {
                println!("If you have lodestone installed to a custom location, please shut down the launcher and set the {} environment variable to the path to your lodestone core installation with `{}`", "LODESTONE_PATH".bold().blue(), "export LODESTONE_PATH=<your path here>".bold().blue());
            }
            // if lodestone_path is not empty, exit
            if lodestone_path.read_dir()?.next().is_some() {
                println!(
                    "{}, this is normal if you ran an older version of lodestone core",
                    format!("Path {} is not empty", lodestone_path.display())
                        .bold()
                        .yellow()
                );
                println!("{} Doing so may break your installation of Lodestone Desktop", "If you have Lodestone Desktop, DO NOT INSTALL A DIFFERENT VERSION OF LODESTONE CORE.".bold().yellow());
                if !yes_all
                    && !prompt_for_confirmation(
                        format!(
                            "Would you like to install lodestone core {} to {}? {}:",
                            new_version.bold().blue(),
                            lodestone_path.display().bold().blue(),
                            "(yes/n)".magenta().bold()
                        ),
                        |s| s.trim() == "yes",
                    )
                {
                    println!("User chose not to install lodestone core, exiting");
                    return Ok(None);
                }
            }
            None
        }
    };
    match version_override {
        None => {
            if let Some(current_version) = current_version {
                println!(
                    "Current version: {}, new version: {}",
                    current_version.bold().blue(),
                    new_version.bold().blue(),
                );
                if current_version == new_version {
                    println!("Current version is new version, skipping update");
                    return Ok(Some(
                        lodestone_path.join(util::get_executable_name(&current_version)?),
                    ));
                }

                if current_version > new_version {
                    println!("Current version is greater than new version, skipping update");
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
                    return Ok(Some(
                        lodestone_path.join(util::get_executable_name(&current_version)?),
                    ));
                }
            }
        }
        Some(v) => println!("Version override: {}", v.bold().yellow()),
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

    println!("Installed lodestone v{}", new_version);
    Ok(Some(executable_path))
}
