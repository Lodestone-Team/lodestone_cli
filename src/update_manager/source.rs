use std::{collections::HashMap, env};
use color_eyre::eyre::Result;

use serde::{Deserialize, Serialize};

use crate::versions::VersionWithV;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Release {
    pub version: VersionWithV,
    pub name: String,
    pub body: String,
    // arch+os -> download url
    pub urls: HashMap<String, String>,
}

impl Release {
    pub fn get_current_arch_os_url(&self) -> Option<&String> {
        let arch_os = format!("{}_{}", env::consts::ARCH, env::consts::OS);
        self.urls.get(&arch_os)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Source {
    name : String,
    description : String,
    releases : HashMap<VersionWithV, Release>,
}

impl Source {
    pub async fn from_url(url: &str) -> Result<Source> {
        // Fetch the source from the url
        let source_json = reqwest::get(url).await?.error_for_status()?.text().await?;
        let source: Source = serde_json::from_str(&source_json)?;
        Ok(source)
    }
    pub fn get_release(&self, version: &VersionWithV) -> Option<&Release> {
        self.releases.get(version)
    }
    pub fn get_latest_release(&self) -> Option<&Release> {
        self.releases.values().max_by_key(|r| r.version.clone())
    }
}