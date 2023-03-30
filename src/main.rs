use chrono::prelude::*;
use std::path::{Path, PathBuf};
mod run_core;
use run_core::run_asset;
mod version_handler;
use version_handler::get_latest_version::get_latest_release;
use version_handler::metadata_handler::update_metadata;
use version_handler::metadata_handler::{read_metadata, Metadata};

mod update_launcher;
use update_launcher::download_release::{download_release, get_path};

use crate::update_launcher::manage_backup::recover_backup;

pub fn update(release_version: &str) -> PathBuf {
    println!("No version found, downloading latest release");
    let (exe_path, exe_file) =
        download_release(release_version).expect("Failed to download latest release");

    let new_metadata = Metadata {
        current_version: release_version.to_string(),
        last_updated: Utc::now().to_string(),
        installer_exe: exe_file,
    };

    match update_metadata(&new_metadata) {
        Ok(_) => println!("Metadata updated: {:?}", new_metadata),
        Err(e) => println!("Error updating metadata: {}", e),
    }

    // let result = run_asset(&exe_path);
    return exe_path;
}
fn main() {
    let metadata_file = Path::new("metadata.json");
    let release_version = match get_latest_release() {
        Ok(v) => v,
        Err(e) => {
            println!("Error: {}", e);
            return;
        }
    };

    // let exe_path = get_path().join(metadata.installer_exe);

    let exe_path: PathBuf = if !metadata_file.exists() {
        update(&release_version)
    } else {
        let metadata = read_metadata();
        let current_version = metadata.current_version;
        if current_version == "" || current_version != release_version {
            update(&release_version)
        } else {
            println!("No update needed");
            get_path().join(metadata.installer_exe)
        }
    };

    let result = run_asset(&exe_path);
    if let Err(e) = result {
        eprintln!("Error in running lodestone core: {}", e);
        recover_backup();
        std::process::exit(1);
    }
}
