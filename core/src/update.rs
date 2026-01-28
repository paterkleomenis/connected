use crate::error::{ConnectedError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub has_update: bool,
    pub latest_version: String,
    pub current_version: String,
    pub download_url: Option<String>,
    pub release_notes: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    body: String,
    assets: Vec<GithubAsset>,
    html_url: String,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

pub struct UpdateChecker;

impl UpdateChecker {
    pub async fn check_for_updates(
        current_version: String,
        platform: String,
    ) -> Result<UpdateInfo> {
        let client = reqwest::Client::builder()
            .user_agent("Connected-App")
            .build()
            .map_err(|e| ConnectedError::Network(e.to_string()))?;

        let url = "https://api.github.com/repos/paterkleomenis/connected/releases/latest";

        let resp = client
            .get(url)
            .send()
            .await
            .map_err(|e| ConnectedError::Network(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(ConnectedError::Network(format!(
                "Failed to fetch updates: {}",
                resp.status()
            )));
        }

        let release: GithubRelease = resp
            .json()
            .await
            .map_err(|e| ConnectedError::Network(format!("Failed to parse release info: {}", e)))?;

        // Parse version (remove 'v' prefix if present)
        let latest_version_str = release.tag_name.trim_start_matches('v');

        let has_update = match (
            semver::Version::parse(&current_version),
            semver::Version::parse(latest_version_str),
        ) {
            (Ok(current), Ok(latest)) => latest > current,
            _ => latest_version_str != current_version,
        };

        let mut download_url = release.html_url.clone();

        // Try to find specific asset based on platform
        // Platform strings: "windows", "linux", "android"
        let suffix = match platform.to_lowercase().as_str() {
            "windows" => ".msi",
            "android" => ".apk",
            "linux" => ".AppImage",
            _ => "",
        };

        if !suffix.is_empty()
            && let Some(asset) = release.assets.iter().find(|a| a.name.ends_with(suffix))
        {
            download_url = asset.browser_download_url.clone();
        }

        Ok(UpdateInfo {
            has_update,
            latest_version: latest_version_str.to_string(),
            current_version,
            download_url: Some(download_url),
            release_notes: Some(release.body),
        })
    }
}
