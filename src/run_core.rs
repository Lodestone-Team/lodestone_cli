use color_eyre::eyre::Result;
use ctrlc::set_handler;
use std::sync::{Arc, Mutex};
use std::{path::PathBuf, process::Command};
use tracing::{error, info};

pub fn run_asset(exe_path: &PathBuf) -> Result<()> {
    info!(
        "Running lodestone core at {}...",
        &exe_path.to_str().unwrap()
    );
    let process = Command::new(exe_path).spawn()?;

    // let pid = process.id();

    // Set up signal handler for CTRL+C
    let process = Arc::new(Mutex::new(process));
    let signal_process = process.clone();
    match set_handler(move || {
        info!("Killing lodestone core...");
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
