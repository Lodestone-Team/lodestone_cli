use color_eyre::eyre::Result;
use semver::Version;
use tracing::info;

use crate::update_manager::metadata::Metadata;
use crate::util;
#[derive(serde::Deserialize)]
struct Release {
    tag_name: String,
}

pub async fn get_latest_release() -> Result<Version> {
    let release_url = "https://api.github.com/repos/Lodestone-Team/lodestone_core/releases/latest";
    let client = reqwest::Client::new();

    let response = client
        .get(release_url)
        .header("User-Agent", "lodestone_launcher")
        .send()
        .await?;

    let release: Release = response.json().await?;
    let latest_version = Version::parse(release.tag_name.trim_start_matches('v'))?;
    info!("Latest version: {}", latest_version);
    Ok(latest_version)
}

pub async fn get_current_version() -> Result<Version> {
    let metadata_path = util::get_metadata_path();
    let metadata = Metadata::read_metadata(&metadata_path).await?;
    let current_version = Version::parse(metadata.current_version.trim_start_matches('v'))?;
    Ok(current_version)
}
