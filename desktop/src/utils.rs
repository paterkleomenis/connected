use arboard::Clipboard;
use std::io::Write;
use std::process::{Command, Stdio};
use tracing::{debug, warn};

fn is_wayland() -> bool {
    std::env::var("XDG_SESSION_TYPE").unwrap_or_default() == "wayland"
}

pub fn get_system_clipboard() -> String {
    if is_wayland() {
        // Try wl-paste first
        match Command::new("wl-paste").arg("--no-newline").output() {
            Ok(output) => {
                if output.status.success() {
                    return String::from_utf8_lossy(&output.stdout).to_string();
                }
            }
            Err(_) => {
                // wl-paste not found or failed, fall back to arboard
            }
        }
    }

    match Clipboard::new() {
        Ok(mut clipboard) => clipboard.get_text().unwrap_or_default(),
        Err(e) => {
            warn!("Failed to access clipboard: {}", e);
            String::new()
        }
    }
}

pub fn set_system_clipboard(text: &str) {
    if is_wayland() {
        // Try wl-copy first
        debug!(
            "Attempting to set clipboard via wl-copy (length: {})",
            text.len()
        );
        match Command::new("wl-copy")
            .arg("--type")
            .arg("text/plain")
            .stdin(Stdio::piped())
            .spawn()
        {
            Ok(mut child) => {
                if let Some(mut stdin) = child.stdin.take() {
                    if let Err(e) = stdin.write_all(text.as_bytes()) {
                        warn!("Failed to write to wl-copy stdin: {}", e);
                    }
                }
                match child.wait() {
                    Ok(status) => {
                        if status.success() {
                            debug!("wl-copy succeeded");
                            return;
                        } else {
                            warn!("wl-copy exited with status: {}", status);
                        }
                    }
                    Err(e) => warn!("Failed to wait on wl-copy: {}", e),
                }
            }
            Err(e) => warn!("Failed to spawn wl-copy: {}", e),
        }
        warn!("wl-copy failed, falling back to arboard");
    }

    match Clipboard::new() {
        Ok(mut clipboard) => {
            if let Err(e) = clipboard.set_text(text) {
                warn!("Failed to set clipboard: {}", e);
            }
        }
        Err(e) => {
            warn!("Failed to access clipboard: {}", e);
        }
    }
}

pub fn get_device_icon(device_type: &str) -> &'static str {
    match device_type.to_lowercase().as_str() {
        "android" => "📱",
        "ios" | "iphone" => "📱",
        "ipad" => "📲",
        "linux" => "🐧",
        "windows" => "🖥️",
        "macos" | "mac" => "💻",
        "tablet" => "📲",
        "desktop" => "🖥️",
        "laptop" => "💻",
        "tv" | "television" => "📺",
        "watch" | "wearable" => "⌚",
        "server" => "🖧",
        _ => "🔌",
    }
}

pub fn get_file_icon(filename: &str) -> &'static str {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "svg" | "bmp" | "ico" => "🖼️",
        "mp4" | "mkv" | "avi" | "mov" | "webm" | "flv" => "🎬",
        "mp3" | "wav" | "flac" | "aac" | "ogg" | "m4a" => "🎵",
        "pdf" => "📕",
        "doc" | "docx" | "odt" => "📝",
        "xls" | "xlsx" | "ods" | "csv" => "📊",
        "ppt" | "pptx" | "odp" => "📽️",
        "zip" | "rar" | "7z" | "tar" | "gz" => "📦",
        "exe" | "msi" | "dmg" | "deb" | "rpm" => "⚙️",
        "txt" | "md" | "rtf" => "📄",
        "html" | "htm" | "css" | "js" | "ts" => "🌐",
        "rs" | "py" | "java" | "c" | "cpp" | "go" | "rb" => "💻",
        "json" | "xml" | "yaml" | "yml" | "toml" => "📋",
        "apk" => "📱",
        _ => "📄",
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
