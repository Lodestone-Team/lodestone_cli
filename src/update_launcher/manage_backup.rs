use crate::update_launcher::download_release::get_lodestone_path;
use color_eyre::eyre::Result;
use std::fs::{self, DirEntry};
use std::path::{Path, PathBuf};
use tracing::{error, info};

pub fn recover_backup() {
    let lodestone_dir = get_lodestone_path();
    let dest_dir = lodestone_dir.join(PathBuf::from(".core_backup"));
    // println!("Copying {:?} to {:?}", dest_dir, lodestone_dir);
    match load_backup(&dest_dir, &lodestone_dir) {
        Ok(_) => info!("Backup loaded"),
        Err(e) => error!("Failed to load backup: {}", e),
    }
}

pub fn copy_dir(source: &Path, destination: &Path) -> Result<()> {
    // println!("Copying {:?} to {:?}", source, destination);
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(&source)? {
        let entry = entry?;
        if is_exe_file(&entry) {
            continue;
        }
        let path = entry.path();
        let file_name = path.file_name().unwrap().to_string_lossy().into_owned(); //unlikely to fail
        if file_name == ".core_backup" {
            continue;
        }
        let dest_path = destination.join(&file_name);

        if path.is_dir() {
            // Recursively copy subdirectories
            copy_dir(&path, &dest_path)?;
        } else {
            // Copy individual files
            fs::copy(&path, &dest_path)?;
        }
    }

    Ok(())
}

pub fn load_backup(backup_path: &Path, current_path: &Path) -> Result<()> {
    if backup_path.exists() {
        info!("Loading backup");
        copy_dir(backup_path, current_path)?;
        fs::remove_dir_all(backup_path)?;
    }
    Ok(())
}

fn is_exe_file(entry: &DirEntry) -> bool {
    if let Some(extension) = entry.path().extension() {
        if extension == "exe" && entry.path().is_file() {
            return true;
        }
    }
    false
}
