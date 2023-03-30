use chrono::prelude::*;
use std::path::Path;
mod version_handler;
use version_handler::get_latest_version::get_latest_release;
use version_handler::metadata_handler::update_metadata;
use version_handler::metadata_handler::{read_metadata, Metadata};

mod installation;
use installation::download_release::download_release;

fn main() {
    let metadata_file = Path::new("metadata.json");

    let metadata = if !metadata_file.exists() {
        let new_metadata = Metadata {
            current_version: "".to_string(),
            last_updated: "".to_string(),
        };
        new_metadata
    } else {
        read_metadata()
    };

    let current_version = metadata.current_version;
    let release_version = match get_latest_release() {
        Ok(v) => v,
        Err(e) => {
            println!("Error: {}", e);
            return;
        }
    };

    if current_version == "" || current_version != release_version {
        println!("No version found, downloading latest release");
        //download latest release
        // get_path();
        download_release(release_version.as_str()).expect("Failed to download latest release");
        //if not successful, restore previous version

        //if successful
        let new_metadata = Metadata {
            current_version: release_version,
            last_updated: Utc::now().to_string(),
        };

        match update_metadata(&new_metadata) {
            Ok(_) => println!("Metadata updated: {:?}", new_metadata),
            Err(e) => println!("Error updating metadata: {}", e),
        }
    }
}
