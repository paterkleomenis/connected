use arboard::Clipboard;
use std::process::Command;
use tracing::warn;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

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

#[cfg(target_os = "windows")]
fn hostname_output() -> std::io::Result<std::process::Output> {
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    Command::new("hostname")
        .creation_flags(CREATE_NO_WINDOW)
        .output()
}

#[cfg(not(target_os = "windows"))]
fn hostname_output() -> std::io::Result<std::process::Output> {
    Command::new("hostname").output()
}

pub fn get_hostname() -> String {
    if let Ok(name) = std::env::var("HOSTNAME") {
        return name;
    }
    if let Ok(name) = std::env::var("COMPUTERNAME") {
        // Windows
        return name;
    }
    if let Ok(output) = hostname_output()
        && output.status.success()
    {
        return String::from_utf8_lossy(&output.stdout).trim().to_string();
    }
    "Desktop".to_string()
}
