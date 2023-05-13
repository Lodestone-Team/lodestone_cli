use color_eyre::eyre::Result;
use color_eyre::owo_colors::OwoColorize;
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
        .header("User-Agent", "lodestone_cli")
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
        .join(".lodestone_cli_metadata.json");
    let metadata = Metadata::read_metadata(&metadata_path).await?;
    Ok(metadata.current_version)
}

pub async fn list_versions() -> Result<()> {
    let release_url = "https://api.github.com/repos/Lodestone-Team/lodestone_core/releases";
    let client = reqwest::Client::new();

    let response = client
        .get(release_url)
        .header("User-Agent", "lodestone_cli")
        .send()
        .await?;
    response.error_for_status_ref()?;
    let releases: Vec<Release> = response.json().await?;
    let mut releases: Vec<Version> = releases
        .iter()
        .map(|release| Version::parse(release.tag_name.as_str()))
        .filter_map(Result::ok)
        .collect();
    releases.sort();
    releases.reverse();
    let current_version = get_current_version().await.ok();
    println!("Available versions:");
    let mut first = true;
    for release in releases {
        if first {
            first = false;
            if let Some(current_version) = &current_version {
                if &release == current_version {
                    println!("  {} (current) (latest)", release.on_blue());
                } else if !release.pre.is_empty() {
                    println!("  {} (latest)", release.yellow());
                }
                continue;
            }
            println!("  {} (latest)", release.green());
        } else if let Some(current_version) = &current_version {
            if &release == current_version {
                println!("  {} (current)", release.on_blue());
            } else if &release < current_version {
                println!("  {}", release.black().strikethrough());
            } else if !release.pre.is_empty() {
                println!("  {}", release.yellow());
            } else {
                println!("  {}", release);
            }
        } else if !release.pre.is_empty() {
            println!("  {}", release.yellow());
        } else {
            println!("  {}", release);
        }
    }

    Ok(())
}
