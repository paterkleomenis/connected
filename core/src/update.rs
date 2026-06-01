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
            // Release workflow publishes this as `connected-desktop.dmg`.
            "macos" => Some(github_release_download_url(&tag, "connected-desktop.dmg")),
            // Prefer AUR on Linux (Arch-based installs), otherwise the AppImage asset.
            "linux" => {
                if is_arch_like() {
                    // Prefer the AUR binary package; the -git variant exists for bleeding edge.
                    Some("https://aur.archlinux.org/packages/connected-desktop-bin".to_string())
                } else {
                    // Release workflow publishes the Linux AppImage.
                    Some(github_release_download_url(
                        &tag,
                        "connected-desktop-x86_64.AppImage",
                    ))
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
            "ID" | "id" if value == "arch" || value == "archlinux" => {
                return true;
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

/// Install a macOS update from a DMG download.
///
/// Downloads the DMG to a temp file, mounts it, copies the .app bundle over the
/// existing installation, unmounts, and relaunches the app.
pub async fn install_macos_update(url: &str) -> Result<()> {
    let dmg_path = PathBuf::from(format!("/tmp/connected-update-{}.dmg", std::process::id()));

    // Download the DMG.
    download_to_file(url, &dmg_path).await?;

    // Find the current .app bundle by walking up from the executable.
    let app_path = find_current_app_bundle()?;

    // Mount the DMG and capture the output to find the mount point.
    let mount_output = tokio::process::Command::new("hdiutil")
        .args([
            "attach",
            dmg_path.to_str().unwrap_or_default(),
            "-nobrowse",
            "-quiet",
            "-plist",
        ])
        .output()
        .await
        .map_err(ConnectedError::Io)?;

    if !mount_output.status.success() {
        let stderr = String::from_utf8_lossy(&mount_output.stderr);
        let _ = tokio::fs::remove_file(&dmg_path).await;
        return Err(ConnectedError::Network(format!(
            "Failed to mount DMG: {}",
            stderr.trim()
        )));
    }

    // Parse the plist output to find the mount point.
    let mount_stdout = String::from_utf8_lossy(&mount_output.stdout);
    let mounted_app = match parse_mount_point_from_plist(&mount_stdout) {
        Some(mp) => mp,
        None => {
            // Fallback: scan /Volumes for the most recently modified .app
            match find_app_in_volumes(Path::new("/Volumes")) {
                Some(app) => app,
                None => {
                    let _ = tokio::fs::remove_file(&dmg_path).await;
                    return Err(ConnectedError::Network(
                        "Could not find .app in mounted DMG".to_string(),
                    ));
                }
            }
        }
    };

    // Extract the volume path (parent of the .app) for unmounting later.
    let volume_path = mounted_app
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf());

    // Copy the new .app over the existing one using ditto (preserves code signing).
    let ditto_status = tokio::process::Command::new("ditto")
        .args([
            mounted_app.to_str().unwrap_or_default(),
            app_path.to_str().unwrap_or_default(),
        ])
        .status()
        .await
        .map_err(ConnectedError::Io)?;

    if !ditto_status.success() {
        // Unmount before returning error.
        if let Some(ref vp) = volume_path {
            let _ = tokio::process::Command::new("hdiutil")
                .args([
                    "detach",
                    vp.to_str().unwrap_or_default(),
                    "-quiet",
                    "-nobrowse",
                ])
                .status()
                .await;
        }
        let _ = tokio::fs::remove_file(&dmg_path).await;
        return Err(ConnectedError::Network(
            "Failed to copy .app bundle".to_string(),
        ));
    }

    // Unmount the DMG.
    if let Some(vp) = volume_path {
        let _ = tokio::process::Command::new("hdiutil")
            .args([
                "detach",
                vp.to_str().unwrap_or_default(),
                "-quiet",
                "-nobrowse",
            ])
            .status()
            .await;
    }

    // Clean up the downloaded DMG.
    let _ = tokio::fs::remove_file(&dmg_path).await;

    // Relaunch the app.
    let _ = tokio::process::Command::new("open")
        .args(["-a", app_path.to_str().unwrap_or_default()])
        .spawn();

    // Exit the current process.
    std::process::exit(0);
}

/// Parse the mount point from hdiutil's XML plist output.
///
/// Looks for a pattern like `<key>mount-point</key><string>/Volumes/Connected</string>`.
fn parse_mount_point_from_plist(plist: &str) -> Option<PathBuf> {
    let mut lines = plist.lines();
    while let Some(line) = lines.next() {
        if line.contains("mount-point") {
            // The next line should contain the path in <string> tags.
            if let Some(next) = lines.next() {
                let start = next.find('<')? + 1;
                let end = next.rfind('>')?;
                if end > start {
                    let mount_point = &next[start..end];
                    // Look for a .app inside the mount point.
                    let mount_dir = Path::new(mount_point);
                    if let Ok(entries) = std::fs::read_dir(mount_dir) {
                        for entry in entries.flatten() {
                            let p = entry.path();
                            if p.extension().and_then(|e| e.to_str()) == Some("app") {
                                return Some(p);
                            }
                        }
                    }
                    // If no .app found directly, return the mount point itself.
                    return Some(mount_dir.to_path_buf());
                }
            }
        }
    }
    None
}

/// Find the current .app bundle by walking up from the executable path.
///
/// On macOS the running binary lives at:
///   /path/to/Connected.app/Contents/MacOS/connected-desktop
/// This walks up to find `Connected.app`.
fn find_current_app_bundle() -> Result<PathBuf> {
    let exe = std::env::current_exe().map_err(ConnectedError::Io)?;
    let mut current = exe.as_path();

    // Walk up looking for a directory ending in .app.
    loop {
        if let Some(name) = current.file_name()
            && name.to_string_lossy().ends_with(".app")
        {
            return Ok(current.to_path_buf());
        }
        current = current.parent().ok_or_else(|| {
            ConnectedError::Io(std::io::Error::other(
                "Could not find .app bundle for current installation",
            ))
        })?;
    }
}

/// Search /Volumes for a just-mounted Connected.app.
fn find_app_in_volumes(volumes_dir: &Path) -> Option<PathBuf> {
    // Give the mount a moment to settle; then scan for the newest .app.
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(volumes_dir) {
        for entry in entries.flatten() {
            let vol_path = entry.path();
            if !vol_path.is_dir() {
                continue;
            }
            if let Ok(app_entries) = std::fs::read_dir(&vol_path) {
                for app_entry in app_entries.flatten() {
                    let p = app_entry.path();
                    if p.extension().and_then(|e| e.to_str()) == Some("app") {
                        candidates.push(p);
                    }
                }
            }
        }
    }
    // Return the most recently modified .app (the one we just mounted).
    candidates.sort_by(|a, b| {
        let ta = std::fs::metadata(a)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        let tb = std::fs::metadata(b)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        tb.cmp(&ta)
    });
    candidates.into_iter().next()
}

/// Install a Linux AppImage update.
///
/// Downloads the new AppImage next to the current one, renames the current
/// to `.old`, renames the new one into place, then relaunches and exits.
pub async fn install_linux_appimage_update(url: &str) -> Result<()> {
    let appimage_path = std::env::var("APPIMAGE")
        .map_err(|_| ConnectedError::Network("Not running as an AppImage".to_string()))?;
    let appimage = PathBuf::from(appimage_path);

    // Download new AppImage to a temp file in the same directory.
    let tmp_path = {
        let file_name = appimage
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("connected-desktop");
        let tmp_name = format!(".{}.tmp.{}", file_name, std::process::id());
        appimage
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(tmp_name)
    };

    download_to_file(url, &tmp_path).await?;

    // Make the new AppImage executable.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&tmp_path, perms).map_err(ConnectedError::Io)?;
    }

    // Rename current AppImage to .old (keeps one backup).
    let old_path = appimage.with_extension("AppImage.old");
    // Remove any leftover .old from a previous update.
    let _ = std::fs::remove_file(&old_path);
    std::fs::rename(&appimage, &old_path).map_err(ConnectedError::Io)?;

    // Rename the new AppImage into the original path.
    std::fs::rename(&tmp_path, &appimage).map_err(ConnectedError::Io)?;

    // Relaunch the new AppImage.
    let _ = tokio::process::Command::new(&appimage).spawn();

    // Exit the current process.
    std::process::exit(0);
}
