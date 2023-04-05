use color_eyre::eyre::Result;
use tracing::info;

#[derive(serde::Deserialize)]
struct Release {
    tag_name: String,
}

pub async fn get_latest_release() -> Result<String> {
    let release_url = "https://api.github.com/repos/Lodestone-Team/lodestone_core/releases/latest";
    let client = reqwest::Client::new();

    let response = client
        .get(release_url)
        .header("User-Agent", "lodestone_launcher")
        .send()
        .await?;

    let release: Release = response.json().await?;
    let latest_version = release.tag_name;
    info!("Latest version: {}", latest_version);
    Ok(latest_version)
}
