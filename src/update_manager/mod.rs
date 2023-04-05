use chrono::Utc;
use color_eyre::eyre::Result;
use std::path::PathBuf;
use tracing::{info, warn};

pub mod download;
pub mod metadata;
pub mod versions;
use crate::{update_manager::download::download_release, util};
use semver::Version;

/// Updates the lodestone core to the latest release if needed
/// Returns the path to the new (or old) executable
pub async fn try_update() -> Result<PathBuf> {
    let current_version = versions::get_current_version()
        .await
        .unwrap_or(Version::new(0, 0, 0));
    let latest_version = versions::get_latest_release().await?;

    let lodestone_path = util::get_lodestone_path();

    if current_version == latest_version {
        return Ok(lodestone_path.join(util::get_executable_name(&current_version)?));
    }

    if current_version > latest_version {
        warn!("Current version is greater than latest release, skipping update");
        return Ok(lodestone_path.join(util::get_executable_name(&current_version)?));
    }

    // Otherwise we need to update

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
