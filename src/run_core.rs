use crate::{error, info};
use color_eyre::eyre::Result;
use color_eyre::owo_colors::OwoColorize;
use ctrlc::set_handler;
use std::sync::{Arc, Mutex};
use std::{path::Path, process::Command};
pub fn run_lodestone(executable_path: &Path) -> Result<()> {
    info!("Running Lodestone Core at {}", &executable_path.display());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(executable_path)?.permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(executable_path, permissions)?;
    }

    let process = Command::new(executable_path).spawn()?;

    // let pid = process.id();

    // Set up signal handler for CTRL+C
    let process = Arc::new(Mutex::new(process));
    let signal_process = process.clone();
    match set_handler(move || {
        info!("Killing Lodestone Core");
        let _ = signal_process.lock().unwrap().kill(); //unlikely to fail
    }) {
        Ok(_) => {}
        Err(e) => {
            error!("Error setting up signal handler: {}", e);
        }
    }

    // Wait for the process to terminate
    let status = process.lock().unwrap().wait()?;
    if !status.success() {
        error!("Process exited with status code: {}", status);
    }

    Ok(())
}
