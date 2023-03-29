use super::temp_backup::{copy_dir, load_backup};
use dirs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::{self};
use std::{env, fs};

use std::time::Duration;
use wait_timeout::ChildExt;

fn get_path() -> PathBuf {
    let home_dir = dirs::home_dir().unwrap();
    println!("Home directory: {:?}", home_dir);
    let lodestone_path = match env::var("LODESTONE_PATH") {
        Ok(val) => PathBuf::from(val),
        Err(_) => home_dir.join(PathBuf::from(".lodestone_launcher")),
    };
    println!("Lodestone path: {:?}", lodestone_path);
    return lodestone_path;
}

fn download_and_run_asset(
    asset_url: &str,
    exe_path: &str,
) -> Result<i32, Box<dyn std::error::Error>> {
    let response = reqwest::blocking::get(asset_url)?;
    let bytes = response.bytes()?;

    let lodestone_dir = get_path();
    std::fs::create_dir_all(&lodestone_dir)?;

    let exe_path = lodestone_dir.join(&exe_path);
    println!("Exe path: {:?}", exe_path);

    fs::write(&exe_path, &bytes)?;
    println!("file written");
    // run the downloaded executable file

    println!("Running executable file...");
    let mut command = Command::new(&exe_path);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn()?;

    let timeout = Duration::from_secs(30);
    let status_code = match child.wait_timeout(timeout).unwrap() {
        Some(status) => status.code(),
        None => {
            // child hasn't exited yet
            println!("Child process timed out, killing it");
            child.kill().unwrap();
            child.wait().unwrap().code()
        }
    };
    println!("Status code: {:?}", status_code.unwrap());

    Ok(1)
}

pub fn download_release(version: &str) -> Result<(), Box<dyn std::error::Error>> {
    let lodestone_dir = get_path();
    let dest_dir = lodestone_dir.join(PathBuf::from(".core_backup"));
    copy_dir(&lodestone_dir, &dest_dir).unwrap_or_else(|e| {
        eprintln!("Failed to copy directory: {}", e);
        std::process::exit(1);
    });

    // Get the target architecture and operating system
    let target_arch = env::consts::ARCH;
    let target_os = env::consts::OS;

    // Choose the appropriate asset filename based on the target architecture and operating system
    let asset_url = match (target_arch, target_os) {
        ("x86_64", "windows") => format!(
            "https://github.com/Lodestone-Team/lodestone_core/releases/download/{}/lodestone_core_windows_{}.exe", version, version),
        ("arm", "linux") => format!(
            "https://github.com/Lodestone-Team/lodestone_core/releases/download/{}/lodestone_core_arm_{}.exe", version, version),
        ("x86", "linux") => format!(
            "https://github.com/Lodestone-Team/lodestone_core/releases/download/{}/lodestone_core_{}.exe", version, version),
        _ => return Err("Unsupported target system".into()),
    };

    let exe_path = match (target_arch, target_os) {
        ("x86_64", "windows") => format!("lodestone_core_windows_{}.exe", version),
        ("arm", "linux") => format!("lodestone_core_arm_{}.exe", version),
        ("x86", "linux") => format!("lodestone_core_{}.exe", version),
        _ => return Err("Unsupported target system".into()),
    };

    let result = download_and_run_asset(&asset_url, &exe_path);
    if let Err(e) = result {
        eprintln!("Failed to download and run asset: {}", e);
        recover_backup(&dest_dir, &lodestone_dir);
        std::process::exit(1);
    } else if result.unwrap_or_default() != 0 {
        eprintln!("Failed to download and run asset");
        recover_backup(&dest_dir, &lodestone_dir);
        std::process::exit(1);
    }

    Ok(())
}

fn recover_backup(dest_dir: &Path, lodestone_dir: &Path) {
    println!("Copying {:?} to {:?}", lodestone_dir, dest_dir);
    match load_backup(&dest_dir, &lodestone_dir) {
        Ok(_) => println!("Backup loaded"),
        Err(e) => eprintln!("Failed to load backup: {}", e),
    }
}
