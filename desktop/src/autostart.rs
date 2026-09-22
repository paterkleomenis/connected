pub const AUTOSTART_ARG: &str = "--autostart";

pub fn is_enabled() -> bool {
    platform::is_enabled()
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    platform::set_enabled(enabled)
}

#[cfg(target_os = "windows")]
mod platform {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    const CREATE_NO_WINDOW: u32 = 0x08000000;
    const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE_NAME: &str = "ConnectedDesktop";

    pub fn is_enabled() -> bool {
        Command::new("reg")
            .creation_flags(CREATE_NO_WINDOW)
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
                .creation_flags(CREATE_NO_WINDOW)
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
            .creation_flags(CREATE_NO_WINDOW)
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

    /// Look up a binary name on PATH without extra dependencies.
    fn find_in_path(name: &str) -> Option<PathBuf> {
        let paths = std::env::var_os("PATH")?;
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    }

    /// Resolve the executable to write into the autostart entry.
    ///
    /// `std::env::current_exe()` returns the *running* binary. When developing
    /// via `cargo run` that is `<repo>/target/{debug,release}/connected-desktop`
    /// (or a `/tmp/.mount-*` path for an AppImage), which stops existing as soon
    /// as the repo moves, `cargo clean` runs, or the user switches to the
    /// installed binary. systemd's autostart generator then skips the entry at
    /// login ("Exec binary ... does not exist: not generating unit") and the app
    /// silently never starts. So for such transient binaries, prefer a stable
    /// `connected-desktop` found on PATH and only fall back to `current_exe()`.
    fn resolve_autostart_exe() -> Result<PathBuf, String> {
        let current = std::env::current_exe()
            .map_err(|e| format!("Failed to resolve executable path: {e}"))?;
        let path_str = current.to_string_lossy();
        let is_transient = path_str.contains("/target/debug/")
            || path_str.contains("/target/release/")
            || path_str.contains("/tmp/.mount");
        if is_transient && let Some(stable) = find_in_path("connected-desktop") {
            return Ok(stable);
        }
        Ok(current)
    }

    /// Extract the executable path from an `Exec=` line: the first token,
    /// honouring double quotes, with `\X` escapes unescaped.
    fn exec_binary(exec_line: &str) -> Option<String> {
        let value = exec_line.strip_prefix("Exec=")?.trim_start();
        if let Some(quoted) = value.strip_prefix('"') {
            let mut binary = String::new();
            let mut chars = quoted.chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    if let Some(escaped) = chars.next() {
                        binary.push(escaped);
                    }
                } else if c == '"' {
                    break;
                } else {
                    binary.push(c);
                }
            }
            if binary.is_empty() {
                None
            } else {
                Some(binary)
            }
        } else {
            let token = value.split_whitespace().next()?;
            if token.is_empty() {
                None
            } else {
                Some(token.to_string())
            }
        }
    }

    fn autostart_target_valid(path: &Path) -> bool {
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(_) => return false,
        };
        let Some(exec_line) = content.lines().find(|line| line.starts_with("Exec=")) else {
            return false;
        };
        let Some(binary) = exec_binary(exec_line) else {
            return false;
        };
        let binary_path = PathBuf::from(&binary);
        if binary_path.is_absolute() {
            return binary_path.is_file();
        }
        // Bare command (e.g. `Exec=connected-desktop`): must resolve on PATH.
        find_in_path(&binary).is_some()
    }

    fn desktop_entry(exe: &Path) -> String {
        format!(
            "[Desktop Entry]\nType=Application\nVersion=1.0\nName=Connected\nComment=High-speed, offline, cross-platform ecosystem bridging devices\nExec={} {}\nIcon=connected-desktop\nTerminal=false\nCategories=Utility;Network;FileTransfer;\nX-GNOME-Autostart-enabled=true\n",
            quote_exec(exe),
            super::AUTOSTART_ARG
        )
    }

    pub fn is_enabled() -> bool {
        match autostart_path() {
            Ok(path) => path.exists() && autostart_target_valid(&path),
            Err(_) => false,
        }
    }

    pub fn set_enabled(enabled: bool) -> Result<(), String> {
        let path = autostart_path()?;

        if enabled {
            let parent = path
                .parent()
                .ok_or_else(|| "Invalid autostart path".to_string())?;
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create autostart directory: {e}"))?;

            let exe = resolve_autostart_exe()?;
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
