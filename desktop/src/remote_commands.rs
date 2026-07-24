//! OS-specific execution of remote power/session commands.
//!
//! These commands are only invoked after a `ConnectedEvent::RemoteCommand` has
//! been received from a trusted peer AND the local "Remote Commands" setting is
//! enabled (see `controller.rs`).

use connected_core::RemoteCommand;
use std::process::Command;
use tracing::{error, info};

/// Execute a remote command on the local machine.
///
/// Runs on the current OS via its native power/session tooling. Failures are
/// logged but never propagated — a failed shutdown shouldn't crash the app.
pub fn execute_remote_command(command: RemoteCommand) {
    info!("Executing remote command: {:?}", command);
    let result = match command {
        RemoteCommand::Shutdown => shutdown(),
        RemoteCommand::Restart => restart(),
        RemoteCommand::SignOut => sign_out(),
        RemoteCommand::OpenUrl(ref url) => open_url(url),
    };
    if let Err(e) = result {
        error!("Failed to execute remote command {:?}: {}", command, e);
    }
}

fn open_url(url: &str) -> std::io::Result<()> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Only http and https URLs can be opened remotely",
        ));
    }

    #[cfg(target_os = "linux")]
    {
        run("xdg-open", &[url])
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(url).spawn().map(|_| ())
    }

    #[cfg(target_os = "windows")]
    {
        run("cmd", &["/C", "start", "", url])
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "remote URL opening is not supported on this platform",
        ))
    }
}

#[cfg(target_os = "linux")]
fn shutdown() -> std::io::Result<()> {
    // `systemctl poweroff` is the canonical systemd shutdown verb.
    run("systemctl", &["poweroff"])
        .or_else(|_| run("poweroff", &[]))
        .or_else(|_| run("shutdown", &["-h", "now"]))
}

#[cfg(target_os = "linux")]
fn restart() -> std::io::Result<()> {
    // `systemctl reboot` is the canonical systemd reboot verb.
    run("systemctl", &["reboot"])
        .or_else(|_| run("reboot", &[]))
        .or_else(|_| run("shutdown", &["-r", "now"]))
}

#[cfg(target_os = "linux")]
fn sign_out() -> std::io::Result<()> {
    // Prefer loginctl terminate-user (valid loginctl verb; works on Wayland/X11).
    if let Ok(user) = std::env::var("USER")
        && run("loginctl", &["terminate-user", &user]).is_ok()
    {
        return Ok(());
    }
    // Fallback for GNOME/X11 sessions.
    run("gnome-session-quit", &["--logout", "--no-prompt"]).or_else(|_| {
        run(
            "qdbus",
            &[
                "org.kde.ksmserver",
                "/KSMServer",
                "org.kde.KSMServerInterface.logout",
                "0",
                "3",
                "0",
            ],
        )
    })
}

#[cfg(target_os = "macos")]
fn shutdown() -> std::io::Result<()> {
    osascript("tell app \"System Events\" to shut down")
}

#[cfg(target_os = "macos")]
fn restart() -> std::io::Result<()> {
    osascript("tell app \"System Events\" to restart")
}

#[cfg(target_os = "macos")]
fn sign_out() -> std::io::Result<()> {
    osascript("tell app \"System Events\" to log out")
}

#[cfg(target_os = "windows")]
fn shutdown() -> std::io::Result<()> {
    run("shutdown", &["/s", "/t", "0"])
}

#[cfg(target_os = "windows")]
fn restart() -> std::io::Result<()> {
    run("shutdown", &["/r", "/t", "0"])
}

#[cfg(target_os = "windows")]
fn sign_out() -> std::io::Result<()> {
    run("shutdown", &["/l"])
}

/// Run a command to completion and return `Err` if it fails to start OR exits
/// with a non-zero status. This lets the `or_else` fallback chain trigger on
/// runtime failures (e.g. an invalid verb or a polkit denial), not just when the
/// binary is missing.
#[cfg(any(target_os = "linux", target_os = "windows"))]
fn run(program: &str, args: &[&str]) -> std::io::Result<()> {
    let status = Command::new(program).args(args).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "{} exited with status {}",
            program,
            status.code().unwrap_or(-1)
        )))
    }
}

#[cfg(target_os = "macos")]
fn osascript(script: &str) -> std::io::Result<()> {
    Command::new("osascript")
        .args(["-e", script])
        .spawn()
        .map(|_| ())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn shutdown() -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "remote commands are not supported on this platform",
    ))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn restart() -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "remote commands are not supported on this platform",
    ))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn sign_out() -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "remote commands are not supported on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_url_rejects_non_http() {
        let err = open_url("file:///etc/passwd").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);

        let err = open_url("ftp://example.com").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }
}
