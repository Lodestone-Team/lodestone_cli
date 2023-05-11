use color_eyre::eyre::Result;
use semver::Version;

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
    response.error_for_status_ref()?;

    let release: Release = response.json().await?;
    let latest_version = Version::parse(release.tag_name.as_str())?;
    Ok(latest_version)
}

pub async fn get_current_version() -> Result<Version> {
    let metadata_path = util::get_lodestone_path()
        .ok_or_else(|| color_eyre::eyre::eyre!("Could not find lodestone path"))?
        .join("metadata.json");
    let metadata = Metadata::read_metadata(&metadata_path).await?;
    Ok(metadata.current_version)
}
