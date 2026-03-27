use dioxus::desktop::DesktopContext;

#[cfg(unix)]
pub async fn listen_for_wakeups(window: DesktopContext) {
    use tokio::net::UnixListener;
    let data_dir = dirs::data_local_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    let connected_dir = data_dir.join("connected");
    let _ = std::fs::create_dir_all(&connected_dir);
    let socket_path = connected_dir.join("single_instance.sock");

    let _ = std::fs::remove_file(&socket_path);
    if let Ok(listener) = UnixListener::bind(&socket_path) {
        while listener.accept().await.is_ok() {
            window.set_visible(true);
            window.set_minimized(false);
            window.set_focus();
        }
    }
}

#[cfg(unix)]
pub fn send_wakeup_signal() {
    use std::os::unix::net::UnixStream;
    let data_dir = dirs::data_local_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    let socket_path = data_dir.join("connected").join("single_instance.sock");

    if let Ok(mut stream) = UnixStream::connect(&socket_path) {
        use std::io::Write;
        let _ = stream.write_all(b"WAKE");
    }
}

#[cfg(windows)]
pub async fn listen_for_wakeups(window: DesktopContext) {
    use tokio::net::windows::named_pipe::ServerOptions;
    const PIPE_NAME: &str = r"\\.\pipe\connected-desktop-single-instance";

    loop {
        let server = match ServerOptions::new().create(PIPE_NAME) {
            Ok(server) => server,
            Err(_) => {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                continue;
            }
        };

        if let Ok(_) = server.connect().await {
            window.set_visible(true);
            window.set_minimized(false);
            window.set_focus();
        }
    }
}

#[cfg(windows)]
pub fn send_wakeup_signal() {
    use std::fs::OpenOptions;
    const PIPE_NAME: &str = r"\\.\pipe\connected-desktop-single-instance";
    if let Ok(mut file) = OpenOptions::new().write(true).open(PIPE_NAME) {
        use std::io::Write;
        let _ = file.write_all(b"WAKE");
    }
}
