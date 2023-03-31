use ctrlc::set_handler;
use std::sync::{Arc, Mutex};
use std::{path::PathBuf, process::Command};

pub fn run_asset(exe_path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "Running lodestone core at {}...",
        &exe_path.to_str().unwrap()
    );
    let process = Command::new(exe_path).spawn()?;

    // let pid = process.id();

    // Set up signal handler for CTRL+C
    let process = Arc::new(Mutex::new(process));
    let signal_process = process.clone();
    match set_handler(move || {
        println!("Killing lodestone core...");
        let _ = signal_process.lock().unwrap().kill(); //unlikely to fail
    }) {
        Ok(_) => {}
        Err(e) => {
            println!("Error setting up signal handler: {}", e);
        }
    }

    // Wait for the process to terminate
    let status = process.lock().unwrap().wait()?;
    if !status.success() {
        eprintln!("Process exited with status code: {}", status);
    }

    Ok(())
}
