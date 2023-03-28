pub fn get_latest_release() -> Result<String, reqwest::Error> {
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
