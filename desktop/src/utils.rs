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
