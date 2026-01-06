use std::process::Command;

pub fn get_system_clipboard() -> String {
    #[cfg(target_os = "linux")]
    {
        if let Ok(output) = Command::new("xclip")
            .args(["-selection", "clipboard", "-o"])
            .output()
        {
            if output.status.success() {
                return String::from_utf8_lossy(&output.stdout).to_string();
            }
        }
        if let Ok(output) = Command::new("wl-paste").output() {
            if output.status.success() {
                return String::from_utf8_lossy(&output.stdout).to_string();
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = Command::new("pbpaste").output() {
            if output.status.success() {
                return String::from_utf8_lossy(&output.stdout).to_string();
            }
        }
    }

    String::new()
}

pub fn set_system_clipboard(text: &str) {
    #[cfg(target_os = "linux")]
    {
        use std::io::Write;
        if let Ok(mut child) = Command::new("xclip")
            .args(["-selection", "clipboard"])
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(stdin) = child.stdin.as_mut() {
                let _ = stdin.write_all(text.as_bytes());
            }
        } else if let Ok(mut child) = Command::new("wl-copy")
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(stdin) = child.stdin.as_mut() {
                let _ = stdin.write_all(text.as_bytes());
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        use std::io::Write;
        if let Ok(mut child) = Command::new("pbcopy")
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(stdin) = child.stdin.as_mut() {
                let _ = stdin.write_all(text.as_bytes());
            }
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
