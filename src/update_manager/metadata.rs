use color_eyre::eyre::Result;
use serde::{Deserialize, Serialize};
use serde_json;
use std::path::Path;
use tokio::fs;
use tokio::io::AsyncWriteExt;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Metadata {
    pub current_version: semver::Version,
    pub last_updated: String,
    pub executable_name: String,
}

impl Metadata {
    pub async fn write_metadata(&self, path: &Path) -> Result<()> {
        let mut file = fs::File::create(path).await?;
        let json = serde_json::to_string(self)?;
        file.write_all(json.as_bytes()).await?;
        Ok(())
    }

    pub async fn read_metadata(path: &Path) -> Result<Metadata> {
        let contents = fs::read_to_string(path).await?;
        let metadata: Metadata = serde_json::from_str(&contents)?;
        Ok(metadata)
    }
}
