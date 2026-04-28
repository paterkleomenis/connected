pub const AUTOSTART_ARG: &str = "--autostart";

pub fn is_enabled() -> bool {
    platform::is_enabled()
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    platform::set_enabled(enabled)
}

#[cfg(target_os = "windows")]
mod platform {
    use std::process::Command;

    const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE_NAME: &str = "ConnectedDesktop";

    pub fn is_enabled() -> bool {
        Command::new("reg")
            .args(["query", RUN_KEY, "/v", VALUE_NAME])
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    pub fn set_enabled(enabled: bool) -> Result<(), String> {
        if enabled {
            let exe = std::env::current_exe()
                .map_err(|e| format!("Failed to resolve executable path: {e}"))?;
            let value = format!("\"{}\" {}", exe.display(), super::AUTOSTART_ARG);

            let status = Command::new("reg")
                .args([
                    "add", RUN_KEY, "/v", VALUE_NAME, "/t", "REG_SZ", "/d", &value, "/f",
                ])
                .status()
                .map_err(|e| format!("Failed to execute registry command: {e}"))?;

            if !status.success() {
                return Err("Failed to enable startup entry in Windows registry".to_string());
            }

            return Ok(());
        }

        if !is_enabled() {
            return Ok(());
        }

        let status = Command::new("reg")
            .args(["delete", RUN_KEY, "/v", VALUE_NAME, "/f"])
            .status()
            .map_err(|e| format!("Failed to execute registry command: {e}"))?;

        if !status.success() {
            return Err("Failed to remove startup entry from Windows registry".to_string());
        }

        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::fs;
    use std::path::{Path, PathBuf};

    const AUTOSTART_FILENAME: &str = "connected-desktop.desktop";

    fn autostart_path() -> Result<PathBuf, String> {
        let config_dir =
            dirs::config_dir().ok_or_else(|| "Unable to resolve config directory".to_string())?;
        Ok(config_dir.join("autostart").join(AUTOSTART_FILENAME))
    }

    fn quote_exec(path: &Path) -> String {
        let path_str = path.display().to_string().replace('"', "\\\"");
        format!("\"{path_str}\"")
    }

    fn desktop_entry(exe: &Path) -> String {
        format!(
            "[Desktop Entry]\nType=Application\nVersion=1.0\nName=Connected\nComment=High-speed, offline, cross-platform ecosystem bridging devices\nExec={} {}\nIcon=connected-desktop\nTerminal=false\nCategories=Utility;Network;FileTransfer;\nX-GNOME-Autostart-enabled=true\n",
            quote_exec(exe),
            super::AUTOSTART_ARG
        )
    }

    pub fn is_enabled() -> bool {
        autostart_path().map(|path| path.exists()).unwrap_or(false)
    }

    pub fn set_enabled(enabled: bool) -> Result<(), String> {
        let path = autostart_path()?;

        if enabled {
            let parent = path
                .parent()
                .ok_or_else(|| "Invalid autostart path".to_string())?;
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create autostart directory: {e}"))?;

            let exe = std::env::current_exe()
                .map_err(|e| format!("Failed to resolve executable path: {e}"))?;
            fs::write(&path, desktop_entry(&exe))
                .map_err(|e| format!("Failed to write autostart desktop entry: {e}"))?;
            return Ok(());
        }

        if path.exists() {
            fs::remove_file(path)
                .map_err(|e| format!("Failed to remove autostart desktop entry: {e}"))?;
        }

        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    const PLIST_FILENAME: &str = "io.connected.desktop.plist";

    fn launch_agent_path() -> Result<PathBuf, String> {
        let home =
            dirs::home_dir().ok_or_else(|| "Unable to resolve home directory".to_string())?;
        Ok(home
            .join("Library")
            .join("LaunchAgents")
            .join(PLIST_FILENAME))
    }

    fn xml_escape(input: &str) -> String {
        input
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;")
    }

    fn launch_agent_plist(exe: &Path) -> String {
        let executable = xml_escape(&exe.display().to_string());
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n    <key>Label</key>\n    <string>io.connected.desktop</string>\n    <key>ProgramArguments</key>\n    <array>\n        <string>{executable}</string>\n        <string>{}</string>\n    </array>\n    <key>RunAtLoad</key>\n    <true/>\n</dict>\n</plist>\n",
            super::AUTOSTART_ARG
        )
    }

    pub fn is_enabled() -> bool {
        launch_agent_path()
            .map(|path| path.exists())
            .unwrap_or(false)
    }

    pub fn set_enabled(enabled: bool) -> Result<(), String> {
        let path = launch_agent_path()?;

        if enabled {
            let parent = path
                .parent()
                .ok_or_else(|| "Invalid launch agent path".to_string())?;
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create LaunchAgents directory: {e}"))?;

            let exe = std::env::current_exe()
                .map_err(|e| format!("Failed to resolve executable path: {e}"))?;
            fs::write(&path, launch_agent_plist(&exe))
                .map_err(|e| format!("Failed to write LaunchAgent plist: {e}"))?;

            let _ = Command::new("launchctl")
                .args(["load", "-w", &path.to_string_lossy()])
                .status();
            return Ok(());
        }

        let _ = Command::new("launchctl")
            .args(["unload", "-w", &path.to_string_lossy()])
            .status();

        if path.exists() {
            fs::remove_file(path)
                .map_err(|e| format!("Failed to remove LaunchAgent plist: {e}"))?;
        }

        Ok(())
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
mod platform {
    pub fn is_enabled() -> bool {
        false
    }

    pub fn set_enabled(_enabled: bool) -> Result<(), String> {
        Err("Autostart is not supported on this platform".to_string())
    }
}
