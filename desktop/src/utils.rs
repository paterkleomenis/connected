use arboard::Clipboard;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tracing::warn;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

/// Name of the store that manages updates for this build, if any.
///
/// Windows builds are updated through the Microsoft Store. Flathub builds are
/// updated by the Flathub repository; installing GitHub release bundles in-app
/// would require the `org.freedesktop.Flatpak` permission, which Flathub
/// applications must not request.
pub fn updates_managed_by() -> Option<&'static str> {
    if cfg!(target_os = "windows") {
        Some("Microsoft Store")
    } else if cfg!(target_os = "linux")
        && connected_core::update::is_installed_via_flatpak()
        && connected_core::update::is_flathub_build()
    {
        Some("Flathub")
    } else {
        None
    }
}

/// Reuse one clipboard connection for the lifetime of the process.
///
/// Creating a new [`Clipboard`] on every poll makes `arboard` retry the
/// Wayland data-control protocol each time. GNOME/Mutter does not implement
/// that protocol, so Flatpak builds fall back to X11; caching keeps that
/// fallback from reconnecting (and logging) every 500 ms.
///
/// A Flatpak manifest must expose `--socket=x11` (with `--share=ipc`) for
/// this fallback. `--socket=fallback-x11` is not enough on a Wayland session.
struct ClipboardCache {
    clipboard: Option<Clipboard>,
    retry_after: Option<Instant>,
}

static CLIPBOARD_CACHE: OnceLock<Mutex<ClipboardCache>> = OnceLock::new();

const CLIPBOARD_RETRY_INTERVAL: Duration = Duration::from_secs(30);

fn with_clipboard<R>(f: impl FnOnce(&mut Clipboard) -> R) -> Option<R> {
    let cache = CLIPBOARD_CACHE.get_or_init(|| {
        Mutex::new(ClipboardCache {
            clipboard: None,
            retry_after: None,
        })
    });
    let mut cache = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    if cache.clipboard.is_none() {
        if cache
            .retry_after
            .is_some_and(|retry_after| retry_after > Instant::now())
        {
            return None;
        }

        match Clipboard::new() {
            Ok(clipboard) => {
                cache.clipboard = Some(clipboard);
                cache.retry_after = None;
            }
            Err(error) => {
                cache.retry_after = Some(Instant::now() + CLIPBOARD_RETRY_INTERVAL);
                warn!("Failed to access clipboard: {}", error);
                return None;
            }
        }
    }

    cache.clipboard.as_mut().map(f)
}

fn reset_clipboard() {
    if let Some(cache) = CLIPBOARD_CACHE.get() {
        let mut cache = cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.clipboard = None;
        cache.retry_after = None;
    }
}

pub fn get_system_clipboard() -> String {
    with_clipboard(|clipboard| clipboard.get_text().unwrap_or_default()).unwrap_or_default()
}

pub fn set_system_clipboard(text: &str) {
    match with_clipboard(|clipboard| clipboard.set_text(text)) {
        Some(Ok(())) => {}
        Some(Err(error)) => {
            warn!("Failed to set clipboard: {}", error);
            reset_clipboard();
        }
        None => {}
    }
}

pub fn format_file_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let i = (bytes as f64).log(1024.0).floor() as usize;
    let i = i.min(UNITS.len() - 1);
    let s = bytes as f64 / 1024.0f64.powi(i as i32);
    format!("{:.1} {}", s, UNITS[i])
}

fn get_pretty_hostname() -> Option<String> {
    #[cfg(target_os = "macos")]
    if let Ok(output) = Command::new("scutil")
        .arg("--get")
        .arg("ComputerName")
        .output()
        && output.status.success()
    {
        let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }

    #[cfg(target_os = "linux")]
    if let Ok(output) = Command::new("hostnamectl").arg("--pretty").output()
        && output.status.success()
    {
        let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }

    #[cfg(target_os = "windows")]
    {
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        if let Ok(output) = Command::new("wmic")
            .args(["computersystem", "get", "name"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            && output.status.success()
        {
            let name = String::from_utf8_lossy(&output.stdout)
                .lines()
                .nth(1)
                .unwrap_or("")
                .trim()
                .to_string();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }

    None
}

pub fn get_hostname() -> String {
    if let Some(name) = get_pretty_hostname() {
        return name;
    }

    #[cfg(target_os = "windows")]
    if let Ok(name) = std::env::var("COMPUTERNAME") {
        return name;
    }

    let mut cmd = Command::new("hostname");
    #[cfg(target_os = "windows")]
    {
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    if let Ok(output) = cmd.output()
        && output.status.success()
    {
        let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !name.is_empty() {
            return name;
        }
    }

    "Desktop".to_string()
}
