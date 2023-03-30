use std::{path::PathBuf, process::Command};

pub fn run_asset(exe_path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "Running lodestone core at {}...",
        &exe_path.to_str().unwrap()
    );
    let _command = Command::new(exe_path).spawn();

    Ok(())
}
