use color_eyre::eyre::Result;
use serde::{Deserialize, Serialize};
use serde_json;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use tracing::error;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Metadata {
    pub current_version: String,
    pub last_updated: String,
    pub installer_exe: String,
}

pub fn read_metadata(path: &Path) -> Metadata {
    let mut contents = String::new();

    match File::open(path) {
        Ok(mut file) => {
            if let Err(e) = file.read_to_string(&mut contents) {
                error!("Error reading metadata file: {}", e);
            }
        }
        Err(e) => {
            error!("Error opening metadata file: {}", e);
        }
    }
    let metadata = match serde_json::from_str(&contents) {
        Ok(m) => m,
        Err(e) => {
            error!("Error parsing metadata file: {}", e);
            Metadata {
                current_version: "0.0.0".to_string(),
                last_updated: "N/A".to_string(),
                installer_exe: "N/A".to_string(),
            }
        }
    };
    return metadata;
}

pub fn update_metadata(metadata: &Metadata) -> Result<()> {
    let mut file = File::create("src/metadata.json")?;
    let json = serde_json::to_string(metadata)?;
    file.write_all(json.as_bytes())?;
    Ok(())
}
