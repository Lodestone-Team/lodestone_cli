use std::fmt::Display;
use std::str::FromStr;

use color_eyre::eyre::Result;
use color_eyre::owo_colors::OwoColorize;
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::update_manager::metadata::Metadata;
use crate::util;
#[derive(serde::Deserialize)]
pub struct Release {
    pub tag_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq, PartialOrd, Ord, Hash)]
pub struct VersionWithV(pub Version);

impl Serialize for VersionWithV {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(format!("v{}", self.0).as_str())
    }
}

impl<'de> Deserialize<'de> for VersionWithV {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(VersionWithV(
            Version::parse(s.trim_start_matches('v')).map_err(serde::de::Error::custom)?,
        ))
    }
}

impl FromStr for VersionWithV {
    type Err = semver::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(VersionWithV(Version::parse(s.trim_start_matches('v'))?))
    }
}

impl Display for VersionWithV {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "v{}", self.0)
    }
}

impl From<Version> for VersionWithV {
    fn from(version: Version) -> Self {
        VersionWithV(version)
    }
}

impl From<VersionWithV> for Version {
    fn from(version: VersionWithV) -> Self {
        version.0
    }
}

pub async fn get_latest_release() -> Result<VersionWithV> {
    let release_url = "https://api.github.com/repos/Lodestone-Team/lodestone_core/releases/latest";
    let client = reqwest::Client::new();

    let response = client
        .get(release_url)
        .header("User-Agent", "lodestone_cli")
        .send()
        .await?;
    response.error_for_status_ref()?;

    let release: Release = response.json().await?;
    let latest_version = VersionWithV::from_str(release.tag_name.as_str())?;
    Ok(latest_version)
}

pub async fn get_current_version() -> Result<VersionWithV> {
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
    let mut releases: Vec<VersionWithV> = releases
        .iter()
        .map(|release| VersionWithV::from_str(release.tag_name.as_str()))
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
                } else if !release.0.pre.is_empty() {
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
            } else if !release.0.pre.is_empty() {
                println!("  {}", release.yellow());
            } else {
                println!("  {}", release);
            }
        } else if !release.0.pre.is_empty() {
            println!("  {}", release.yellow());
        } else {
            println!("  {}", release);
        }
    }

    Ok(())
}
