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

        let platform_lower = platform.to_lowercase();
        let download_url = match platform_lower.as_str() {
            "android" => select_asset(
                &release.assets,
                ".apk",
                &["android", "universal", "arm64", "aarch64"],
            )
            .map(|asset| asset.browser_download_url.clone()),
            "windows" => select_asset(&release.assets, ".msi", &["x64", "amd64", "win"])
                .map(|asset| asset.browser_download_url.clone())
                .or_else(|| Some(release.html_url.clone())),
            "linux" => select_asset(&release.assets, ".appimage", &["x64", "amd64"])
                .map(|asset| asset.browser_download_url.clone())
                .or_else(|| Some(release.html_url.clone())),
            _ => Some(release.html_url.clone()),
        };

        Ok(UpdateInfo {
            has_update,
            latest_version: latest_version_str.to_string(),
            current_version,
            download_url,
            release_notes: Some(release.body),
        })
    }
}

fn select_asset<'a>(
    assets: &'a [GithubAsset],
    suffix: &str,
    preferred_tokens: &[&str],
) -> Option<&'a GithubAsset> {
    let suffix_lower = suffix.to_lowercase();
    let mut candidates: Vec<&GithubAsset> = assets
        .iter()
        .filter(|asset| asset.name.to_lowercase().ends_with(&suffix_lower))
        .collect();

    if candidates.is_empty() {
        return None;
    }

    for token in preferred_tokens {
        if let Some(asset) = candidates
            .iter()
            .find(|asset| asset.name.to_lowercase().contains(token))
        {
            return Some(*asset);
        }
    }

    candidates.sort_by_key(|asset| asset.name.len());
    candidates.first().copied()
}
