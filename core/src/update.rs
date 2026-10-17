use crate::error::{ConnectedError, Result};
use reqwest::header::LOCATION;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// Minisign public key (base64, as printed by `minisign -G` or
/// `rsign generate`) whose secret key signs every release's `SHA256SUMS`
/// manifest.
///
/// This constant is the update trust anchor: only checksum manifests signed by
/// the matching secret key are accepted, and release CI refuses to publish a
/// release whose signature does not verify against **this** exact key — so the
/// key shipped inside the binary and the key used to sign releases can never
/// silently drift apart.
///
/// Initial setup:
/// 1. `minisign -G -W -p connected-update.pub -s connected-update.key`
/// 2. Store the *secret* key file contents as the `MINISIGN_SECRET_KEY`
///    repository secret.
/// 3. Paste the base64 public key (second line of `connected-update.pub`)
///    below.
///
/// Rotation requires shipping one release that users install manually (old
/// binaries only trust the old key), then updating both this constant and the
/// `MINISIGN_SECRET_KEY` secret.
pub const UPDATE_MINISIGN_PUBKEY: &str = "RWS5fvhebqr2k1ftgxwI5t78tRjGa1Mb+JK/zBiMxS1z4aEdgXfYMd81";

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
            // Fallback for non-semver strings: normalize (trim, drop a leading
            // 'v') before comparing so "v3.2.7" == "3.2.7" doesn't report a
            // phantom update.
            _ => {
                fn normalize_version(s: &str) -> &str {
                    s.trim().trim_start_matches('v')
                }
                normalize_version(latest_version_str).to_lowercase()
                    != normalize_version(&current_version).to_lowercase()
            }
        };

        let platform_lower = platform.to_lowercase();
        let download_url = match platform_lower.as_str() {
            // Release workflow publishes this as `connected-android.apk`.
            "android" => Some(github_release_download_url(&tag, "connected-android.apk")),
            // Release workflow publishes this as `connected-desktop.msi`.
            "windows" => Some(github_release_download_url(&tag, "connected-desktop.msi")),
            // Release workflow publishes this as `connected-desktop-macos.dmg`.
            "macos" => Some(github_release_download_url(
                &tag,
                "connected-desktop-macos.dmg",
            )),
            // On Linux, determine the update source based on how the app was installed.
            "linux" => {
                let arch = std::env::consts::ARCH;
                if is_running_as_appimage() {
                    Some(github_release_download_url(
                        &tag,
                        &format!("connected-desktop-{arch}.AppImage"),
                    ))
                } else if is_installed_via_flatpak() {
                    Some(github_release_download_url(
                        &tag,
                        &format!("com.paterkleomenis.Connected-{arch}.flatpak"),
                    ))
                } else if is_installed_via_aur_async().await {
                    Some("https://aur.archlinux.org/packages/connected-desktop-bin".to_string())
                } else {
                    Some(github_release_download_url(
                        &tag,
                        &format!("connected-desktop-linux-{arch}"),
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

/// Detect whether the app is running as an AppImage.
///
/// AppImage sets the `APPIMAGE` environment variable to the path of the
/// running AppImage file.
pub fn is_running_as_appimage() -> bool {
    std::env::var("APPIMAGE").is_ok()
}

/// Detect whether the app is installed as a Flatpak.
///
/// Checks for the presence of `/.flatpak-info` (created inside every Flatpak
/// sandbox) or the `FLATPAK_ID` environment variable.
pub fn is_installed_via_flatpak() -> bool {
    std::path::Path::new("/.flatpak-info").exists() || std::env::var("FLATPAK_ID").is_ok()
}

/// Detect whether the app is installed via the AUR.
///
/// Runs `pacman -Q connected-desktop` and checks the exit code. If the
/// package is installed, pacman exits 0.
///
/// Since the checks for AppImage and Flatpak run first, we can reasonably assume
/// that if the AUR check succeeds, it is the intended choice for updating.
pub fn is_installed_via_aur() -> bool {
    std::process::Command::new("pacman")
        .args(["-Q", "connected-desktop"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Async variant of [`is_installed_via_aur`] for async contexts.
/// Uses `tokio::process::Command` so the executor thread is not blocked
/// while `pacman` runs.
pub async fn is_installed_via_aur_async() -> bool {
    tokio::process::Command::new("pacman")
        .args(["-Q", "connected-desktop"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Download an update asset to a local file path (streamed; avoids buffering the full payload).
///
/// NOTE: performs **no** verification. Release assets must go through the
/// internal `download_release_asset`, which enforces the release's signed
/// checksums before anything is installed.
///
/// Writes to a temporary file first, then atomically renames to `dest_path` to
/// prevent partial/corrupt downloads from being used if the process crashes or
/// the network drops mid-transfer. On Unix the temp file is created with
/// owner-only permissions (0600).
pub async fn download_to_file(url: &str, dest_path: &Path) -> Result<()> {
    download_to_file_internal(url, dest_path, None).await
}

/// Download and verify SHA256 checksum if provided.
/// If `expected_sha256` is Some, the hex digest is verified before the atomic rename.
/// The check is case-insensitive and ignores surrounding whitespace.
pub async fn download_to_file_verified(
    url: &str,
    dest_path: &Path,
    expected_sha256: &str,
) -> Result<()> {
    download_to_file_internal(url, dest_path, Some(expected_sha256)).await
}

/// Internal download with optional SHA256 verification and collision-free temp name.
async fn download_to_file_internal(
    url: &str,
    dest_path: &Path,
    expected_sha256: Option<&str>,
) -> Result<()> {
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
    // Use pid + uuid to avoid collisions when two concurrent downloads target the same dest.
    let tmp_path: PathBuf = {
        let file_name = dest_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("download");
        let tmp_name = format!(
            ".{}.tmp.{}-{}",
            file_name,
            std::process::id(),
            uuid::Uuid::new_v4()
        );
        dest_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(tmp_name)
    };

    // Scope the file so it is closed (and flushed) before the rename.
    let download_result: Result<Option<String>> = async {
        let mut file = tokio::fs::File::create(&tmp_path).await?;

        // Restrict temp file permissions to owner-only on Unix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            let _ = tokio::fs::set_permissions(&tmp_path, perms).await;
        }

        // Optionally compute SHA256 while streaming
        let mut hasher = if expected_sha256.is_some() {
            use sha2::{Digest, Sha256};
            Some(Sha256::new())
        } else {
            None
        };

        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| ConnectedError::Network(e.to_string()))?
        {
            if let Some(ref mut h) = hasher {
                use sha2::Digest;
                h.update(&chunk);
            }
            file.write_all(&chunk).await?;
        }

        file.flush().await?;
        file.sync_all().await?;
        let hex = hasher.map(|h| {
            use sha2::Digest;
            h.finalize()
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect::<String>()
        });
        Ok(hex)
    }
    .await;

    let computed_hash = match download_result {
        Ok(h) => h,
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(e);
        }
    };

    // Verify checksum if requested
    if let (Some(expected), Some(computed)) = (expected_sha256, computed_hash.as_deref()) {
        let expected_norm = expected.trim().to_ascii_lowercase();
        let computed_norm = computed.trim().to_ascii_lowercase();
        if expected_norm != computed_norm {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(ConnectedError::ChecksumMismatch);
        }
    }

    // Atomic rename into place.
    if let Err(e) = tokio::fs::rename(&tmp_path, dest_path).await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(ConnectedError::Io(e));
    }

    Ok(())
}

/// Base URL of the release a download asset belongs to.
///
/// GitHub release asset URLs are
/// `https://github.com/<owner>/<repo>/releases/download/<tag>/<file>`; every
/// artifact of that release — `SHA256SUMS`, its signature, all downloadable
/// assets — lives in the same directory. Deriving the base URL from the asset
/// keeps verification pinned to the asset's own release: a signed manifest
/// from any other release is never consulted.
fn release_base_url(asset_url: &str) -> Result<&str> {
    asset_url
        .rsplit_once('/')
        .map(|(base, _)| base)
        .ok_or_else(|| {
            ConnectedError::UpdateVerification(format!("Malformed release asset URL: {asset_url}"))
        })
}

/// Fetch the release's `SHA256SUMS` manifest plus its minisign signature,
/// verify the signature against the pinned [`UPDATE_MINISIGN_PUBKEY`], and
/// return the parsed `file name -> sha256 hex` map.
///
/// Fails closed: a missing artifact, invalid signature, or malformed manifest
/// is an error. There is deliberately no unverified fallback.
async fn fetch_release_checksums(asset_url: &str) -> Result<HashMap<String, String>> {
    let base = release_base_url(asset_url)?;
    let client = reqwest::Client::builder()
        .user_agent("Connected-App")
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| ConnectedError::Network(e.to_string()))?;

    let manifest =
        fetch_release_text(&client, &format!("{base}/SHA256SUMS"), "checksum manifest").await?;
    let signature = fetch_release_text(
        &client,
        &format!("{base}/SHA256SUMS.minisig"),
        "checksum signature",
    )
    .await?;
    verify_and_parse_checksums(&manifest, &signature, UPDATE_MINISIGN_PUBKEY)
}

/// Fetch a small text artifact from the release page. Errors explain that
/// auto-update is disabled for releases without signed checksums.
async fn fetch_release_text(client: &reqwest::Client, url: &str, what: &str) -> Result<String> {
    let response = client.get(url).send().await.map_err(|e| {
        ConnectedError::UpdateVerification(format!(
            "Could not fetch the release {what} ({url}): {e}. \
             Auto-update is disabled for releases without signed checksums."
        ))
    })?;
    if !response.status().is_success() {
        return Err(ConnectedError::UpdateVerification(format!(
            "Release {what} is unavailable ({url}: HTTP {}). \
             Auto-update is disabled for releases without signed checksums.",
            response.status()
        )));
    }
    response.text().await.map_err(|e| {
        ConnectedError::UpdateVerification(format!("Failed to read the release {what}: {e}"))
    })
}

/// Verify a minisign signature over a `SHA256SUMS` manifest and parse it.
///
/// Kept separate from the network fetch so the crypto and parsing paths are
/// unit-testable against committed fixtures.
fn verify_and_parse_checksums(
    manifest: &str,
    signature: &str,
    public_key: &str,
) -> Result<HashMap<String, String>> {
    let key = minisign_verify::PublicKey::from_base64(public_key).map_err(|e| {
        ConnectedError::UpdateVerification(format!(
            "This build has no valid update signing key configured ({e}); \
             refusing to install unverified updates"
        ))
    })?;
    let signature = minisign_verify::Signature::decode(signature).map_err(|e| {
        ConnectedError::UpdateVerification(format!("Invalid update signature format: {e}"))
    })?;
    // `allow_legacy = true` accepts both pre-hashed (modern) and legacy
    // signatures; both are Ed25519 signatures over the same manifest bytes.
    key.verify(manifest.as_bytes(), &signature, true)
        .map_err(|e| {
            ConnectedError::UpdateVerification(format!(
                "Update checksum signature does not verify with the pinned update key: {e}"
            ))
        })?;

    let checksums = parse_sha256sums(manifest);
    if checksums.is_empty() {
        return Err(ConnectedError::UpdateVerification(
            "Update checksum manifest contains no entries".to_string(),
        ));
    }
    Ok(checksums)
}

/// Parse `sha256sum`-style output: `<64-hex>␣␣<name>` (text mode) or
/// `<64-hex>␣*<name>` (binary mode). Blank and malformed lines are skipped.
fn parse_sha256sums(manifest: &str) -> HashMap<String, String> {
    let mut checksums = HashMap::new();
    for line in manifest.lines() {
        let Some((hash, rest)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let name = rest.trim_start().trim_start_matches('*');
        if name.is_empty() {
            continue;
        }
        if hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
            checksums.insert(name.to_string(), hash.to_ascii_lowercase());
        }
    }
    checksums
}

/// Download a release asset, failing closed unless the downloaded bytes match
/// the signature-verified checksum for that file.
///
/// This is the only download path update installers should use; the raw
/// [`download_to_file`] performs no verification.
async fn download_release_asset(url: &str, dest_path: &Path) -> Result<()> {
    let file_name = url.rsplit('/').next().unwrap_or_default();
    let checksums = fetch_release_checksums(url).await?;
    let expected = checksums.get(file_name).ok_or_else(|| {
        ConnectedError::UpdateVerification(format!(
            "Signed checksum manifest has no entry for {file_name}; refusing to install"
        ))
    })?;
    download_to_file_verified(url, dest_path, expected).await
}

/// Install a macOS update from a DMG download.
///
/// Downloads the DMG to a temp file, mounts it, copies the .app bundle over the
/// existing installation, unmounts, and relaunches the app.
pub async fn install_macos_update(url: &str) -> Result<()> {
    let dmg_path = PathBuf::from(format!(
        "/tmp/connected-update-{}-{}.dmg",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));

    // Download the DMG. Verification is mandatory: the install refuses to
    // proceed unless the image matches the release's signed checksums.
    download_release_asset(url, &dmg_path).await?;

    // Find the current .app bundle by walking up from the executable.
    let app_path = find_current_app_bundle()?;

    // Mount the DMG at a private, freshly created mount point. Using an
    // explicit mount point (instead of parsing `hdiutil -plist` output and
    // falling back to scanning /Volumes for the newest .app) guarantees the
    // bundle we copy comes from *this* image and not from another mounted
    // volume.
    let mount_dir = PathBuf::from(format!(
        "/tmp/connected-update-mount-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    tokio::fs::create_dir_all(&mount_dir)
        .await
        .map_err(ConnectedError::Io)?;

    let mount_output = tokio::process::Command::new("hdiutil")
        .args([
            "attach",
            dmg_path.to_str().unwrap_or_default(),
            "-nobrowse",
            "-quiet",
            "-mountpoint",
            mount_dir.to_str().unwrap_or_default(),
        ])
        .output()
        .await
        .map_err(ConnectedError::Io)?;

    if !mount_output.status.success() {
        let stderr = String::from_utf8_lossy(&mount_output.stderr);
        let _ = tokio::fs::remove_dir_all(&mount_dir).await;
        let _ = tokio::fs::remove_file(&dmg_path).await;
        return Err(ConnectedError::Network(format!(
            "Failed to mount DMG: {}",
            stderr.trim()
        )));
    }

    // Find the .app bundle inside the mounted image.
    let mut mounted_app = None;
    if let Ok(mut entries) = tokio::fs::read_dir(&mount_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let candidate = entry.path();
            if candidate.extension().and_then(|e| e.to_str()) == Some("app") {
                mounted_app = Some(candidate);
                break;
            }
        }
    }
    let Some(mounted_app) = mounted_app else {
        detach_dmg(&mount_dir).await;
        let _ = tokio::fs::remove_file(&dmg_path).await;
        return Err(ConnectedError::Network(
            "Could not find .app in mounted DMG".to_string(),
        ));
    };

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
        detach_dmg(&mount_dir).await;
        let _ = tokio::fs::remove_file(&dmg_path).await;
        return Err(ConnectedError::Network(
            "Failed to copy .app bundle".to_string(),
        ));
    }

    // Unmount the DMG.
    detach_dmg(&mount_dir).await;

    // Clean up the downloaded DMG.
    let _ = tokio::fs::remove_file(&dmg_path).await;

    // Relaunch the app.
    let _ = tokio::process::Command::new("open")
        .args(["-a", app_path.to_str().unwrap_or_default()])
        .spawn();

    // Exit the current process.
    std::process::exit(0);
}

/// Detach a mounted update DMG and remove its private mount point.
///
/// Best-effort: called on both success and error paths, so it never fails the
/// caller.
async fn detach_dmg(mount_dir: &Path) {
    let _ = tokio::process::Command::new("hdiutil")
        .args([
            "detach",
            mount_dir.to_str().unwrap_or_default(),
            "-quiet",
            "-nobrowse",
        ])
        .status()
        .await;
    let _ = tokio::fs::remove_dir_all(mount_dir).await;
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
        let tmp_name = format!(
            ".{}.tmp.{}-{}",
            file_name,
            std::process::id(),
            uuid::Uuid::new_v4()
        );
        appimage
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(tmp_name)
    };

    // Verification is mandatory: refuse to replace the AppImage unless the
    // download matches the release's signed checksums.
    download_release_asset(url, &tmp_path).await?;

    // Make the new AppImage executable.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        tokio::fs::set_permissions(&tmp_path, perms)
            .await
            .map_err(ConnectedError::Io)?;
    }

    // Rename current AppImage to .old (keeps one backup).
    let old_path = appimage.with_extension("AppImage.old");
    // Remove any leftover .old from a previous update.
    let _ = tokio::fs::remove_file(&old_path).await;
    tokio::fs::rename(&appimage, &old_path)
        .await
        .map_err(ConnectedError::Io)?;

    // Rename the new AppImage into the original path.
    tokio::fs::rename(&tmp_path, &appimage)
        .await
        .map_err(ConnectedError::Io)?;

    // Relaunch the new AppImage.
    let _ = tokio::process::Command::new(&appimage).spawn();

    // Exit the current process.
    std::process::exit(0);
}

/// Install a Linux Flatpak update.
///
/// Downloads the Flatpak bundle, installs it matching the current installation
/// type (system-wide installation or user), relaunches and terminates the old app.
pub async fn install_linux_flatpak_update(url: &str) -> Result<()> {
    let downloads_dir = dirs::download_dir().unwrap_or_else(|| {
        PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()))
    });
    let bundle_path = downloads_dir.join(format!(
        "connected-update-{}-{}.flatpak",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));

    // Verification is mandatory: refuse to install the bundle unless the
    // download matches the release's signed checksums.
    download_release_asset(url, &bundle_path).await?;

    let bundle_str = bundle_path.to_str().unwrap_or_default();

    let is_system_installation = tokio::process::Command::new("flatpak-spawn")
        .args([
            "--host",
            "flatpak",
            "info",
            "--system",
            "com.paterkleomenis.Connected",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);

    // System installs need pkexec for polkit authentication
    let install_status = if is_system_installation {
        tokio::process::Command::new("flatpak-spawn")
            .args([
                "--host",
                "pkexec",
                "flatpak",
                "install",
                "--system",
                "--reinstall",
                "--bundle",
                "-y",
                bundle_str,
            ])
            .status()
            .await
            .map_err(ConnectedError::Io)?
    } else {
        tokio::process::Command::new("flatpak-spawn")
            .args([
                "--host",
                "flatpak",
                "install",
                "--user",
                "--reinstall",
                "--bundle",
                "-y",
                bundle_str,
            ])
            .status()
            .await
            .map_err(ConnectedError::Io)?
    };

    let _ = tokio::fs::remove_file(&bundle_path).await;

    if !install_status.success() {
        return Err(ConnectedError::Network(
            "Flatpak install failed".to_string(),
        ));
    }

    let _ = tokio::process::Command::new("flatpak-spawn")
        .args(["--host", "flatpak", "run", "com.paterkleomenis.Connected"])
        .spawn();

    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_PUBKEY_FILE: &str = include_str!("../tests/fixtures/update/test.pub");
    const TEST_MANIFEST: &str = include_str!("../tests/fixtures/update/SHA256SUMS");
    const TEST_SIGNATURE: &str = include_str!("../tests/fixtures/update/SHA256SUMS.minisig");

    /// The base64 key line from the fixture's `.pub` file.
    fn test_pubkey() -> &'static str {
        TEST_PUBKEY_FILE
            .lines()
            .find(|line| !line.starts_with("untrusted comment") && !line.trim().is_empty())
            .expect("fixture public key has a base64 line")
            .trim()
    }

    #[test]
    fn verifies_signed_manifest_and_parses_entries() {
        let checksums = verify_and_parse_checksums(TEST_MANIFEST, TEST_SIGNATURE, test_pubkey())
            .expect("fixture signature must verify");
        assert_eq!(
            checksums
                .get("connected-desktop-x86_64.AppImage")
                .map(String::as_str),
            Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        );
        assert_eq!(checksums.len(), 3);
    }

    #[test]
    fn rejects_tampered_manifest() {
        let tampered = TEST_MANIFEST.replace(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        );
        assert_ne!(tampered, TEST_MANIFEST);
        let err = verify_and_parse_checksums(&tampered, TEST_SIGNATURE, test_pubkey())
            .expect_err("tampered manifest must not verify");
        assert!(matches!(err, ConnectedError::UpdateVerification(_)));
    }

    #[test]
    fn rejects_manifest_signed_by_a_different_key() {
        // A syntactically valid minisign public key that is not the fixture's.
        let other_key = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
        let err = verify_and_parse_checksums(TEST_MANIFEST, TEST_SIGNATURE, other_key)
            .expect_err("a different key must not verify the fixture signature");
        assert!(matches!(err, ConnectedError::UpdateVerification(_)));
    }

    #[test]
    fn invalid_pinned_key_fails_closed() {
        // Also covers the un-configured placeholder value: a build that has no
        // real update key must refuse every manifest instead of installing.
        let err = verify_and_parse_checksums(
            TEST_MANIFEST,
            TEST_SIGNATURE,
            "REPLACE_WITH_MINISIGN_PUBLIC_KEY",
        )
        .expect_err("an unconfigured update key must refuse to verify anything");
        assert!(matches!(err, ConnectedError::UpdateVerification(_)));
    }

    #[test]
    fn pinned_pubkey_is_a_valid_minisign_key() {
        // Release CI only publishes manifests that verify against the key
        // compiled into the binary; a malformed key would break auto-update
        // for every user, so fail at test time instead.
        minisign_verify::PublicKey::from_base64(UPDATE_MINISIGN_PUBKEY)
            .expect("UPDATE_MINISIGN_PUBKEY must be a valid minisign public key");
    }

    #[test]
    fn parses_sha256sums_in_text_and_binary_modes() {
        let manifest = concat!(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  file one.bin\r\n",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb *binary.bin\n",
            "DDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDD  upper.bin\n",
            "not a hash line\n",
            "\n",
            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc\n",
        );
        let checksums = parse_sha256sums(manifest);
        assert_eq!(checksums.len(), 3);
        assert_eq!(
            checksums.get("file one.bin").map(String::as_str),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        assert_eq!(
            checksums.get("binary.bin").map(String::as_str),
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
        );
        assert_eq!(
            checksums.get("upper.bin").map(String::as_str),
            Some("dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd")
        );
    }

    #[test]
    fn derives_release_base_url_from_asset_url() {
        assert_eq!(
            release_base_url(
                "https://github.com/o/r/releases/download/v1.2.3/connected-desktop-x86_64.AppImage"
            )
            .unwrap(),
            "https://github.com/o/r/releases/download/v1.2.3"
        );
        assert!(release_base_url("garbage").is_err());
    }
}
