use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use dioxus::desktop::tao::window::Window;
use once_cell::sync::Lazy;

static WAKEUP_REQUESTED: AtomicBool = AtomicBool::new(false);
static WAKEUP_LISTENER_STARTED: AtomicBool = AtomicBool::new(false);
static LAST_WAKEUP_MARKER: AtomicU64 = AtomicU64::new(0);
static WAKEUP_WINDOW: Lazy<Mutex<Option<Arc<Window>>>> = Lazy::new(|| Mutex::new(None));

fn connected_data_dir() -> PathBuf {
    let data_dir = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
    data_dir.join("connected")
}

fn wakeup_marker_path() -> PathBuf {
    connected_data_dir().join("wakeup.signal")
}

fn current_wakeup_marker_value() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros()
        .min(u64::MAX as u128) as u64
}

fn write_wakeup_marker() -> u64 {
    let connected_dir = connected_data_dir();
    let _ = std::fs::create_dir_all(&connected_dir);
    let marker = current_wakeup_marker_value();
    let _ = std::fs::write(wakeup_marker_path(), marker.to_string());
    marker
}

fn read_wakeup_marker() -> Option<u64> {
    let raw = std::fs::read_to_string(wakeup_marker_path()).ok()?;
    raw.trim().parse::<u64>().ok()
}

pub fn request_wakeup() {
    WAKEUP_REQUESTED.store(true, Ordering::Release);
    let marker = write_wakeup_marker();
    LAST_WAKEUP_MARKER.store(marker, Ordering::Release);
    wake_current_window();
}

pub fn take_wakeup_request() -> bool {
    if WAKEUP_REQUESTED.swap(false, Ordering::AcqRel) {
        if let Some(marker) = read_wakeup_marker() {
            LAST_WAKEUP_MARKER.store(marker, Ordering::Release);
        }
        return true;
    }

    let marker = match read_wakeup_marker() {
        Some(marker) => marker,
        None => return false,
    };

    let last = LAST_WAKEUP_MARKER.load(Ordering::Acquire);
    if marker <= last {
        return false;
    }

    LAST_WAKEUP_MARKER.store(marker, Ordering::Release);
    true
}

pub fn initialize_wakeup_state() {
    if let Some(marker) = read_wakeup_marker() {
        LAST_WAKEUP_MARKER.store(marker, Ordering::Release);
    }
}

pub fn set_wakeup_window(window: Arc<Window>) {
    if let Ok(mut current) = WAKEUP_WINDOW.lock() {
        *current = Some(window.clone());
    }

    if WAKEUP_REQUESTED.load(Ordering::Acquire) {
        restore_window(window);
    }
}

pub fn show_window(window: &Window) {
    window.set_visible(true);

    #[cfg(target_os = "linux")]
    {
        // Ensure the window is not minimized when restoring.
        // Some Linux WMs might treat hidden windows as minimized.
        window.set_minimized(false);
        window.set_focus();
    }

    #[cfg(not(target_os = "linux"))]
    window.set_focus();
}

fn restore_window(window: Arc<Window>) {
    #[cfg(target_os = "linux")]
    {
        glib::MainContext::default().invoke(move || {
            show_window(&window);

            let focus_window = window.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(75), move || {
                focus_window.set_focus();
            });
        });
    }

    #[cfg(not(target_os = "linux"))]
    {
        show_window(&window);
    }
}

fn wake_current_window() {
    let window = WAKEUP_WINDOW
        .lock()
        .ok()
        .and_then(|current| current.as_ref().cloned());

    if let Some(window) = window {
        restore_window(window);
    }
}

pub fn quit_application() -> ! {
    std::process::exit(0);
}

pub fn start_wakeup_listener() {
    if WAKEUP_LISTENER_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }

    std::thread::Builder::new()
        .name("ipc-wakeup".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();

            if let Ok(runtime) = runtime {
                runtime.block_on(async move {
                    listen_for_wakeups().await;
                });
            }
        })
        .ok();
}

#[cfg(unix)]
pub async fn listen_for_wakeups() {
    use tokio::net::UnixListener;
    let connected_dir = connected_data_dir();
    let _ = std::fs::create_dir_all(&connected_dir);
    let socket_path = connected_dir.join("single_instance.sock");

    let _ = std::fs::remove_file(&socket_path);
    if let Ok(listener) = UnixListener::bind(&socket_path) {
        while listener.accept().await.is_ok() {
            request_wakeup();
        }
    }
}

#[cfg(unix)]
pub fn send_wakeup_signal() {
    use std::os::unix::net::UnixStream;
    use std::time::Duration;
    request_wakeup();
    let socket_path = connected_data_dir().join("single_instance.sock");

    for _ in 0..10 {
        if let Ok(mut stream) = UnixStream::connect(&socket_path) {
            use std::io::Write;
            let _ = stream.write_all(b"WAKE");
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(windows)]
pub async fn listen_for_wakeups() {
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

        if server.connect().await.is_ok() {
            request_wakeup();
        }
    }
}

#[cfg(windows)]
pub fn send_wakeup_signal() {
    use std::fs::OpenOptions;
    use std::time::Duration;
    request_wakeup();
    const PIPE_NAME: &str = r"\\.\pipe\connected-desktop-single-instance";
    for _ in 0..10 {
        if let Ok(mut file) = OpenOptions::new().write(true).open(PIPE_NAME) {
            use std::io::Write;
            let _ = file.write_all(b"WAKE");
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
