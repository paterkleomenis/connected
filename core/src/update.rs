use crate::error::{ConnectedError, Result};
use reqwest::header::LOCATION;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub has_update: bool,
    pub latest_version: String,
    pub current_version: String,
    pub download_url: Option<String>,
    pub release_notes: Option<String>,
}

const REPO_OWNER: &str = "paterkleomenis";
const REPO_NAME: &str = "connected";
const GITHUB_RELEASES_LATEST_URL: &str =
    "https://github.com/paterkleomenis/connected/releases/latest";

pub struct UpdateChecker;

impl UpdateChecker {
    pub async fn check_for_updates(
        current_version: String,
        platform: String,
    ) -> Result<UpdateInfo> {
        let client = reqwest::Client::builder()
            .user_agent("Connected-App")
            // We want the redirect target (tag) without pulling the HTML page.
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| ConnectedError::Network(e.to_string()))?;

        // Avoid GitHub API rate limiting by resolving the latest tag via the releases redirect.
        // We intentionally do not follow redirects so we can parse the tag from the Location header.
        let resp = client
            .get(GITHUB_RELEASES_LATEST_URL)
            .send()
            .await
            .map_err(|e| ConnectedError::Network(e.to_string()))?;

        let (tag, html_url) = if resp.status().is_redirection() {
            let location = resp
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| {
                    ConnectedError::Network(
                        "Latest release redirect missing Location header".to_string(),
                    )
                })?;
            let url = absolutize_github_location(location);
            parse_tag_from_release_url(&url).ok_or_else(|| {
                ConnectedError::Network("Failed to parse latest release tag".to_string())
            })?
        } else if resp.status().is_success() {
            // Fallback: if GitHub ever stops redirecting, try parsing the response URL directly.
            parse_tag_from_release_url(resp.url().as_str()).ok_or_else(|| {
                ConnectedError::Network("Failed to parse latest release tag".to_string())
            })?
        } else {
            return Err(ConnectedError::Network(format!(
                "Failed to resolve latest release: {}",
                resp.status()
            )));
        };

        // Parse version (remove 'v' prefix if present)
        let latest_version_str = tag.trim_start_matches('v');

        let has_update = match (
            semver::Version::parse(&current_version),
            semver::Version::parse(latest_version_str),
        ) {
            (Ok(current), Ok(latest)) => latest > current,
            _ => latest_version_str != current_version,
        };

        let platform_lower = platform.to_lowercase();
        let download_url = match platform_lower.as_str() {
            // Release workflow publishes this as `connected-android.apk`.
            "android" => Some(github_release_download_url(&tag, "connected-android.apk")),
            // Release workflow publishes this as `connected-desktop.msi`.
            "windows" => Some(github_release_download_url(&tag, "connected-desktop.msi")),
            // Prefer AUR on Linux (Arch-based installs), otherwise fall back to a raw binary asset.
            "linux" => {
                if is_arch_like() {
                    // Prefer the AUR binary package; the -git variant exists for bleeding edge.
                    Some("https://aur.archlinux.org/packages/connected-desktop-bin".to_string())
                } else {
                    // Release workflow publishes the Linux binary as `connected-desktop` (no extension).
                    Some(github_release_download_url(&tag, "connected-desktop"))
                }
            }
            _ => Some(html_url),
        };

        Ok(UpdateInfo {
            has_update,
            latest_version: latest_version_str.to_string(),
            current_version,
            download_url,
            // Intentionally omitted here: parsing HTML is fragile and the API is rate-limited.
            release_notes: None,
        })
    }
}

fn parse_tag_from_release_url(url: &str) -> Option<(String, String)> {
    // Expected final URL:
    //   https://github.com/<owner>/<repo>/releases/tag/<tag>
    let tag = url.split("/releases/tag/").nth(1)?.trim_matches('/');
    if tag.is_empty() {
        return None;
    }
    Some((tag.to_string(), url.to_string()))
}

fn absolutize_github_location(location: &str) -> String {
    // GitHub commonly redirects with a relative Location.
    if location.starts_with("http://") || location.starts_with("https://") {
        location.to_string()
    } else {
        format!("https://github.com{}", location)
    }
}

fn github_release_download_url(tag: &str, asset_name: &str) -> String {
    format!(
        "https://github.com/{}/{}/releases/download/{}/{}",
        REPO_OWNER, REPO_NAME, tag, asset_name
    )
}

fn is_arch_like() -> bool {
    // Best-effort detection so we can prefer AUR without breaking other distros.
    // Properly parse /etc/os-release as KEY=VALUE pairs per the freedesktop spec
    // instead of fragile substring matching that could false-positive on unrelated
    // values (e.g. a PRETTY_NAME containing "arch" in another word).
    let Ok(contents) = std::fs::read_to_string("/etc/os-release") else {
        return false;
    };

    for line in contents.lines() {
        let line = line.trim();
        let Some((key, raw_value)) = line.split_once('=') else {
            continue;
        };

        // Strip optional quotes around the value (single or double).
        let value = raw_value
            .trim_matches('"')
            .trim_matches('\'')
            .to_lowercase();

        match key {
            "ID" | "id" => {
                if value == "arch" || value == "archlinux" {
                    return true;
                }
            }
            "ID_LIKE" | "id_like" => {
                // ID_LIKE can be a space-separated list of distro identifiers.
                for token in value.split_whitespace() {
                    if token == "arch" || token == "archlinux" {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }

    false
}

/// Download an update asset to a local file path (streamed; avoids buffering the full payload).
///
/// Writes to a temporary file first, then atomically renames to `dest_path` to
/// prevent partial/corrupt downloads from being used if the process crashes or
/// the network drops mid-transfer. On Unix the temp file is created with
/// owner-only permissions (0600).
pub async fn download_to_file(url: &str, dest_path: &Path) -> Result<()> {
    let client = reqwest::Client::builder()
        .user_agent("Connected-App")
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| ConnectedError::Network(e.to_string()))?;

    let mut resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| ConnectedError::Network(e.to_string()))?;

    if !resp.status().is_success() {
        return Err(ConnectedError::Network(format!(
            "Failed to download update: {}",
            resp.status()
        )));
    }

    // Write to a temp file in the same directory so rename is atomic (same filesystem).
    let tmp_path: PathBuf = {
        let file_name = dest_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("download");
        let tmp_name = format!(".{}.tmp.{}", file_name, std::process::id());
        dest_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(tmp_name)
    };

    // Scope the file so it is closed (and flushed) before the rename.
    let download_result: Result<()> = async {
        let mut file = tokio::fs::File::create(&tmp_path).await?;

        // Restrict temp file permissions to owner-only on Unix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            let _ = std::fs::set_permissions(&tmp_path, perms);
        }

        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| ConnectedError::Network(e.to_string()))?
        {
            file.write_all(&chunk).await?;
        }

        file.flush().await?;
        file.sync_all().await?;
        Ok(())
    }
    .await;

    if let Err(e) = download_result {
        // Best-effort cleanup of the temp file on failure.
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(e);
    }

    // Atomic rename into place.
    if let Err(e) = tokio::fs::rename(&tmp_path, dest_path).await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(ConnectedError::Io(e));
    }

    Ok(())
}
