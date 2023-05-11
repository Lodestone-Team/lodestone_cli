use std::path::Path;

use color_eyre::eyre::Result;

pub fn uninstall(lodestone_path: &Path) -> Result<()> {
    std::fs::remove_dir_all(lodestone_path)?;
    Ok(())
}
