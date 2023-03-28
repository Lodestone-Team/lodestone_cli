use dirs;
use std::path::PathBuf;
use std::process::Command;
use std::thread::{self};
use std::{env, fs};

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

pub fn download_release(version: &str) -> Result<(), Box<dyn std::error::Error>> {
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

    let exe_path = get_path().join(format!("lodestone_core_windows_{}.exe", version));
    let handle = thread::spawn(move || {
        let response = reqwest::blocking::get(asset_url).expect("Failed to download release");
        let bytes = response.bytes().expect("Failed to read bytes");

        let lodestone_dir = get_path();

        fs::create_dir_all(&lodestone_dir).expect("Failed to create directory");

        let exe_path = lodestone_dir.join(exe_path);
        println!("Exe path: {:?}", exe_path);
        fs::write(&exe_path, &bytes).expect("Failed to write file");
        println!("file written");
        // run the downloaded executable file
        let output = Command::new(&exe_path)
            .output()
            .expect("Failed to execute file");
        println!("{:?}", output);
    });

    handle.join().expect("Failed to join thread");
    Ok(())
}
