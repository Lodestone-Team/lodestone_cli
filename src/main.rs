use chrono::prelude::*;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json;
use std::fs::File;
use std::io::{copy, BufWriter};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use zip::ZipArchive;

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Metadata {
    current_version: String,
    last_updated: String,
}

fn download_release(
    version: &str,
    download_path: &str,
    extract_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // Build the download URL
    let url = format!(
        "https://github.com/Lodestone-Team/lodestone_core/archive/{}.zip",
        version
    );

    // Create a reqwest client
    let client = Client::new();

    // Download the release archive
    let mut response = client.get(&url).send()?;
    let response_body = response.text()?;
    println!("Response body: {:?}", response_body);

    // Save the archive to a file
    let file_path = Path::new(download_path);
    let mut file = BufWriter::new(File::create(&file_path)?);
    // copy(&mut response, &mut file)?;

    // Extract the archive to a folder
    let extract_path = Path::new(extract_path);
    let file = File::open(file_path)?;
    let mut archive = ZipArchive::new(file)?;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let outpath = extract_path.join(file.sanitized_name());
        if (&*file.name()).ends_with('/') {
            std::fs::create_dir_all(&outpath)?;
        } else {
            if let Some(p) = outpath.parent() {
                if !p.exists() {
                    std::fs::create_dir_all(&p)?;
                }
            }
            let mut outfile = File::create(&outpath)?;
            std::io::copy(&mut file, &mut outfile)?;
        }
    }

    Ok(())
}

fn get_latest_release() -> Result<String, reqwest::Error> {
    let release_url = "https://api.github.com/repos/Lodestone-Team/lodestone_core/releases/latest";
    let client = reqwest::blocking::Client::new();
    let response = client
        .get(release_url)
        .header("User-Agent", "lodestone_launcher")
        .send()
        .expect("Failed to get latest release");
    // println!("{:?}", response.text())
    let release: serde_json::Value = response.json().expect("Failed to parse response as JSON");
    // println!("{:?}", release);
    let latest_version = release["tag_name"]
        .as_str()
        .expect("Failed to get latest version");
    println!("Latest version: {}", latest_version);
    return Ok(latest_version.to_string());
}

fn read_metadata() -> Metadata {
    let path = Path::new("metadata.json");

    let mut file = File::open("metadata.json").expect("Failed to open metadata file");
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .expect("Failed to read metadata file");
    // println!("{:?}", file.read_to_string(&mut contents).unwrap());
    let metadata: Metadata = serde_json::from_str(&contents).expect("Failed to parse metadata");
    println!("{:?}", metadata);
    // let current_version = metadata.current_version;
    // println!("Current version: {}", current_version);
    return metadata;
    // let metadata = serde_json::from_str(&contents)?;
    // println!("{:?}", metadata);
}

fn main() {
    let metadata_file = Path::new("metadata.json");

    let metadata = if !metadata_file.exists() {
        let new_metadata = Metadata {
            current_version: "".to_string(),
            last_updated: "".to_string(),
        };
        new_metadata
    } else {
        read_metadata()
    };

    let current_version = metadata.current_version;
    let release_version = get_latest_release();

    if let Err(e) = release_version {
        println!("Error: {}", e);
        return;
    }

    if current_version == "" || current_version != release_version.as_ref().unwrap().to_string() {
        println!("No version found, downloading latest release");
        //download latest release
        download_release(
            release_version.as_ref().unwrap().as_str(),
            "latest.zip",
            "latest",
        )
        .expect("Failed to download latest release");
        //if not successful, restore previous version
        //if successful
        let new_metadata = Metadata {
            current_version: release_version.as_ref().unwrap().to_string(),
            last_updated: Utc::now().to_string(),
        };

        println!("{:?}", new_metadata);

        let mut file = File::create("metadata.json").unwrap();
        let json = serde_json::to_string(&new_metadata).unwrap();
        file.write_all(json.as_bytes()).unwrap();
    }
}
