use chrono::prelude::*;
use std::path::{Path, PathBuf};
use tracing_subscriber::{EnvFilter, FmtSubscriber};
mod run_core;
use run_core::run_asset;
mod version_handler;
use version_handler::get_latest_version::get_latest_release;
use version_handler::metadata_handler::update_metadata;
use version_handler::metadata_handler::{read_metadata, Metadata};

mod update_launcher;
use update_launcher::download_release::{download_release, get_lodestone_path};

use crate::update_launcher::manage_backup::recover_backup;
use tracing::{debug, error, info};

pub fn update(release_version: &str) -> PathBuf {
    let (exe_path, exe_file) = match download_release(release_version) {
        Ok((p, f)) => (p, f),
        Err(e) => {
            error!("Failed to download latest release: {}", e);
            return PathBuf::from("");
        }
    };

    let new_metadata = Metadata {
        current_version: release_version.to_string(),
        last_updated: Utc::now().to_string(),
        executable_name: exe_file,
    };

    match update_metadata(&new_metadata) {
        Ok(_) => debug!("Metadata updated: {:?}", new_metadata),
        Err(e) => error!("Error updating metadata: {}", e),
    }

    // let result = run_asset(&exe_path);
    return exe_path;
}

#[tokio::main]
async fn main() {
    color_eyre::install().unwrap();
    // env::set_var("RUST_LOG", "warn");
    let env_filter = EnvFilter::from_default_env()
        .add_directive(tracing::level_filters::LevelFilter::INFO.into());
    let subscriber = FmtSubscriber::builder()
        .with_env_filter(env_filter)
        .finish();

    match tracing::subscriber::set_global_default(subscriber) {
        Ok(_) => info!("Subscriber set"),
        Err(e) => error!("Error setting subscriber: {}", e),
    }

    let metadata_file = Path::new("src/metadata.json");
    let release_version = match get_latest_release().await {
        Ok(v) => v,
        Err(e) => {
            error!("Error: {}", e);
            return;
        }
    };

    // let exe_path = get_path().join(metadata.executable_name);

    let exe_path: PathBuf = if !metadata_file.exists() {
        info!("No metadata file found, downloading latest release");
        update(&release_version)
    } else {
        let metadata = read_metadata(&metadata_file);
        let current_version = metadata.current_version;
        if current_version == "" || current_version != release_version {
            info!("New version found, downloading latest release");
            update(&release_version)
        } else {
            info!("No update needed");
            get_lodestone_path().join(metadata.executable_name)
        }
    };

    let result = run_asset(&exe_path);
    if let Err(e) = result {
        error!("Error in running lodestone core: {}", e);
        recover_backup();
    };
}
