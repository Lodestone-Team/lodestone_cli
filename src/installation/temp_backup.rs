use std::fs;
use std::io::Error;
use std::path::Path;

pub fn copy_dir(source: &Path, destination: &Path) -> Result<(), Error> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(&source)? {
        let entry = entry?;
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

pub fn load_backup(backup_path: &Path, current_path: &Path) -> Result<(), Error> {
    if backup_path.exists() {
        println!("Loading backup");
        copy_dir(backup_path, current_path)?;
        fs::remove_dir_all(backup_path)?;
    }
    Ok(())
}
