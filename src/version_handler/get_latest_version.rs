pub fn get_latest_release() -> Result<String, anyhow::Error> {
    let release_url = "https://api.github.com/repos/Lodestone-Team/lodestone_core/releases/latest";
    let client = reqwest::blocking::Client::new();
    let response = client
        .get(release_url)
        .header("User-Agent", "lodestone_launcher")
        .send()?;

    let release: serde_json::Value = response.json()?;
    let latest_version = match release["tag_name"].as_str() {
        Some(version) => version.to_string(),
        None => {
            return Err(anyhow::anyhow!("Failed to get latest version"));
        }
    };
    println!("Latest version: {}", latest_version);
    Ok(latest_version.to_string())
}
