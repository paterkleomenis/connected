use arboard::Clipboard;
use tracing::warn;

pub fn get_system_clipboard() -> String {
    match Clipboard::new() {
        Ok(mut clipboard) => clipboard.get_text().unwrap_or_default(),
        Err(e) => {
            warn!("Failed to access clipboard: {}", e);
            String::new()
        }
    }
}

pub fn set_system_clipboard(text: &str) {
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
        "ios" | "iphone" | "ipad" => "📱",
        "linux" => "🐧",
        "windows" => "💻",
        "macos" | "mac" => "🍎",
        "tablet" => "📲",
        _ => "💻",
    }
}

#[allow(dead_code)]
pub fn format_file_size(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1} GB", bytes as f64 / 1_000_000_000.0)
    } else if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.1} KB", bytes as f64 / 1_000.0)
    } else {
        format!("{} B", bytes)
    }
}
