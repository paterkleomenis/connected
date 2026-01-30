#![windows_subsystem = "windows"]
#![allow(non_snake_case)]

mod components;
mod controller;
mod fs_provider;
mod mpris_server;
mod proximity;
mod state;
mod utils;

use state::{
    get_clipboard_sync_enabled, get_device_name_setting, get_media_enabled_setting,
    set_clipboard_sync_enabled, set_media_enabled_setting,
};

use components::{DeviceCard, FileBrowser, FileDialog, Icon, IconType};
use connected_core::telephony::{ActiveCallState, CallAction};
use connected_core::{MediaCommand, UpdateInfo};
use controller::{AppAction, app_controller};
use dioxus::desktop::use_window;
use dioxus::prelude::*;

use state::*;
use std::path::PathBuf;
use std::time::Duration;
use tracing::debug;
use utils::{get_hostname, get_system_clipboard};

fn format_timestamp(ts: u64) -> String {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let diff_secs = (now.saturating_sub(ts)) / 1000;

    if diff_secs < 60 {
        "Just now".to_string()
    } else if diff_secs < 3600 {
        format!("{}m ago", diff_secs / 60)
    } else if diff_secs < 86400 {
        format!("{}h ago", diff_secs / 3600)
    } else if diff_secs < 604800 {
        format!("{}d ago", diff_secs / 86400)
    } else {
        let secs = ts / 1000;
        let datetime = UNIX_EPOCH + Duration::from_secs(secs);
        if let Ok(dur) = datetime.duration_since(UNIX_EPOCH) {
            let days = dur.as_secs() / 86400;
            let years = 1970 + days / 365;
            let remaining = days % 365;
            let months = remaining / 30 + 1;
            let day = remaining % 30 + 1;
            format!("{}/{}/{}", months, day, years % 100)
        } else {
            "Unknown".to_string()
        }
    }
}

#[cfg(target_os = "linux")]
mod tray {
    use dioxus::desktop::tao::window::Window;
    use std::sync::Arc;

    pub struct ConnectedTray {
        pub window: Arc<Window>,
    }

    impl ksni::Tray for ConnectedTray {
        fn id(&self) -> String {
            "connected-desktop".to_string()
        }

        fn title(&self) -> String {
            "Connected".to_string()
        }

        fn icon_name(&self) -> String {
            "connected-app-logo".to_string()
        }

        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            // Generate the Connected logo icon
            let width = 64i32;
            let height = 64i32;
            let mut argb = Vec::with_capacity((width * height * 4) as usize);

            for y in 0..height {
                for x in 0..width {
                    let scale = 64.0 / 116.0;
                    let cx = 58.0 * scale;
                    let cy = 58.0 * scale;
                    let dot_radius = 9.0 * scale;
                    let arc_radius = 38.0 * scale;
                    let stroke_width = 16.0 * scale;
                    let half_stroke = stroke_width / 2.0;

                    let px = x as f32;
                    let py = y as f32;

                    let dist = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();

                    let mut alpha: u8 = 0;

                    // Dot
                    if dist <= dot_radius {
                        alpha = 255;
                    } else if dist <= dot_radius + 1.0 {
                        alpha = (255.0 * (1.0 - (dist - dot_radius))) as u8;
                    }

                    // Arc
                    if alpha == 0 {
                        let dist_from_arc = (dist - arc_radius).abs();

                        if dist_from_arc <= half_stroke + 1.0 {
                            let angle = (py - cy).atan2(px - cx);
                            let gap_start = 0.608;
                            let gap_end = 2.533;

                            if !(angle > gap_start && angle < gap_end) {
                                if dist_from_arc <= half_stroke {
                                    alpha = 255;
                                } else {
                                    alpha = (255.0 * (1.0 - (dist_from_arc - half_stroke))) as u8;
                                }
                            }
                        }
                    }

                    // ARGB format (big-endian: A, R, G, B)
                    argb.push(alpha);
                    argb.push(255); // R (white)
                    argb.push(255); // G (white)
                    argb.push(255); // B (white)
                }
            }

            vec![ksni::Icon {
                width,
                height,
                data: argb,
            }]
        }

        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            use ksni::menu::*;
            vec![
                StandardItem {
                    label: "Show Connected".to_string(),
                    activate: Box::new(|tray: &mut Self| {
                        tray.window.set_visible(true);
                        tray.window.set_minimized(false);
                        tray.window.set_focus();
                    }),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "Hide Connected".to_string(),
                    activate: Box::new(|tray: &mut Self| {
                        tray.window.set_visible(false);
                    }),
                    ..Default::default()
                }
                .into(),
                MenuItem::Separator,
                StandardItem {
                    label: "Quit".to_string(),
                    activate: Box::new(|_: &mut Self| {
                        std::process::exit(0);
                    }),
                    ..Default::default()
                }
                .into(),
            ]
        }
    }
}

use image::ImageReader;
use std::io::Cursor;

fn load_icon() -> dioxus::desktop::tao::window::Icon {
    let icon_bytes = include_bytes!("../assets/logo.png");
    let reader = ImageReader::new(Cursor::new(icon_bytes))
        .with_guessed_format()
        .expect("Failed to detect icon format");
    let image = reader.decode().expect("Failed to decode icon");
    let rgba = image.into_rgba8();
    let (width, height) = rgba.dimensions();
    dioxus::desktop::tao::window::Icon::from_rgba(rgba.into_raw(), width, height)
        .expect("Failed to create icon")
}

#[cfg(target_os = "windows")]
fn load_tray_icon() -> dioxus::desktop::trayicon::Icon {
    let icon_bytes = include_bytes!("../assets/logo.png");
    let reader = ImageReader::new(Cursor::new(icon_bytes))
        .with_guessed_format()
        .expect("Failed to detect icon format");
    let image = reader.decode().expect("Failed to decode icon");
    let rgba = image.into_rgba8();
    let (width, height) = rgba.dimensions();
    dioxus::desktop::trayicon::Icon::from_rgba(rgba.into_raw(), width, height)
        .expect("Failed to create tray icon")
}

#[cfg(target_os = "windows")]
fn ensure_firewall_rules() {
    use std::env;
    use windows_firewall::{
        ActionFirewallWindows, DirectionFirewallWindows, ProtocolFirewallWindows,
        WindowsFirewallRule,
    };

    let exe_path = match env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Failed to get current exe path for firewall rules: {}", e);
            return;
        }
    };
    let exe_path_str = exe_path.to_string_lossy();

    // mDNS Inbound: allow receiving mDNS responses and queries on port 5353
    let mdns_inbound = WindowsFirewallRule::builder()
        .name("Connected Desktop (mDNS) - Inbound")
        .action(ActionFirewallWindows::Allow)
        .direction(DirectionFirewallWindows::In)
        .enabled(true)
        .description("Allow Connected Desktop to receive mDNS traffic for local network discovery.")
        .protocol(ProtocolFirewallWindows::Udp)
        .local_ports([5353])
        .application_name(exe_path_str)
        .build();

    if let Err(e) = mdns_inbound.add_or_update() {
        tracing::warn!("Failed to add/update mDNS inbound firewall rule: {}", e);
    }

    // mDNS Outbound: allow sending mDNS queries to port 5353
    let mdns_outbound = WindowsFirewallRule::builder()
        .name("Connected Desktop (mDNS) - Outbound")
        .action(ActionFirewallWindows::Allow)
        .direction(DirectionFirewallWindows::Out)
        .enabled(true)
        .description("Allow Connected Desktop to send mDNS queries for local network discovery.")
        .protocol(ProtocolFirewallWindows::Udp)
        .remote_ports([5353])
        .application_name(exe_path_str)
        .build();

    if let Err(e) = mdns_outbound.add_or_update() {
        tracing::warn!("Failed to add/update mDNS outbound firewall rule: {}", e);
    }

    // QUIC/UDP Inbound: allow incoming connections on any port for this application
    // This is needed so other devices can connect to our dynamically-assigned QUIC port
    let quic_inbound = WindowsFirewallRule::builder()
        .name("Connected Desktop (QUIC) - Inbound")
        .action(ActionFirewallWindows::Allow)
        .direction(DirectionFirewallWindows::In)
        .enabled(true)
        .description("Allow Connected Desktop to receive incoming connections from paired devices.")
        .protocol(ProtocolFirewallWindows::Udp)
        .application_name(exe_path_str)
        .build();

    if let Err(e) = quic_inbound.add_or_update() {
        tracing::warn!("Failed to add/update QUIC inbound firewall rule: {}", e);
    }

    // QUIC/UDP Outbound: allow outgoing connections on any port for this application
    let quic_outbound = WindowsFirewallRule::builder()
        .name("Connected Desktop (QUIC) - Outbound")
        .action(ActionFirewallWindows::Allow)
        .direction(DirectionFirewallWindows::Out)
        .enabled(true)
        .description("Allow Connected Desktop to connect to paired devices.")
        .protocol(ProtocolFirewallWindows::Udp)
        .application_name(exe_path_str)
        .build();

    if let Err(e) = quic_outbound.add_or_update() {
        tracing::warn!("Failed to add/update QUIC outbound firewall rule: {}", e);
    }
}

#[cfg(target_os = "windows")]
fn ensure_webview2_runtime_available() {
    use std::path::PathBuf;
    use std::process::Command;

    fn runtime_installed() -> bool {
        let mut roots: Vec<PathBuf> = Vec::new();

        if let Some(p) = std::env::var_os("ProgramFiles(x86)") {
            roots.push(
                PathBuf::from(p)
                    .join("Microsoft")
                    .join("EdgeWebView")
                    .join("Application"),
            );
        }

        if let Some(p) = std::env::var_os("LOCALAPPDATA") {
            roots.push(
                PathBuf::from(p)
                    .join("Microsoft")
                    .join("EdgeWebView")
                    .join("Application"),
            );
        }

        for root in roots {
            if root.join("msedgewebview2.exe").exists() {
                return true;
            }

            let Ok(entries) = std::fs::read_dir(&root) else {
                continue;
            };

            for entry in entries.flatten() {
                let candidate = entry.path().join("msedgewebview2.exe");
                if candidate.exists() {
                    return true;
                }
            }
        }

        false
    }

    fn show_error(msg: &str) -> ! {
        let _ = rfd::MessageDialog::new()
            .set_title("Connected")
            .set_description(msg)
            .set_level(rfd::MessageLevel::Error)
            .show();
        std::process::exit(1);
    }

    if runtime_installed() {
        return;
    }

    let Ok(exe) = std::env::current_exe() else {
        show_error(
            "Connected requires Microsoft Edge WebView2 Runtime.\n\nInstall 'Microsoft Edge WebView2 Runtime' (Evergreen) and try again.",
        );
    };

    let Some(dir) = exe.parent() else {
        show_error(
            "Connected requires Microsoft Edge WebView2 Runtime.\n\nInstall 'Microsoft Edge WebView2 Runtime' (Evergreen) and try again.",
        );
    };

    let bootstrapper = dir.join("MicrosoftEdgeWebView2Setup.exe");
    if bootstrapper.exists() {
        match Command::new(&bootstrapper)
            .args(["/silent", "/install"])
            .status()
        {
            Ok(status) if status.success() => {
                // Give it a brief moment to lay down files before re-checking.
                std::thread::sleep(std::time::Duration::from_secs(2));
                if runtime_installed() {
                    return;
                }
                // Installation reported success but runtime not found
                show_error(
                    "WebView2 Runtime installation completed but runtime is not available.\n\nPlease restart your computer and try again, or install 'Microsoft Edge WebView2 Runtime' (Evergreen) manually.",
                );
            }
            Ok(status) => {
                // Bootstrapper ran but returned error (likely no admin rights)
                show_error(&format!(
                    "WebView2 Runtime installation failed (exit code: {}).\n\nPlease run Connected as Administrator, or install 'Microsoft Edge WebView2 Runtime' (Evergreen) manually and try again.",
                    status
                ));
            }
            Err(e) => {
                // Could not execute bootstrapper
                show_error(&format!(
                    "Failed to run WebView2 installer: {}\n\nPlease install 'Microsoft Edge WebView2 Runtime' (Evergreen) manually and try again.",
                    e
                ));
            }
        }
    } else {
        show_error(
            "Connected requires Microsoft Edge WebView2 Runtime.\n\nInstall 'Microsoft Edge WebView2 Runtime' (Evergreen) and try again.\n\nIf you used a debloat tool that removes WebView2/Edge components, exclude WebView2 Runtime from removal.",
        );
    }
}

#[cfg(target_os = "windows")]
fn install_windows_panic_hook() {
    use std::fmt::Write as _;
    use std::io::Write as _;
    use std::time::{SystemTime, UNIX_EPOCH};

    std::panic::set_hook(Box::new(move |info| {
        let mut msg = String::new();
        let _ = writeln!(&mut msg, "Connected crashed.");

        if let Some(location) = info.location() {
            let _ = writeln!(
                &mut msg,
                "Location: {}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            );
        }

        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "<non-string panic payload>".to_string()
        };
        let _ = writeln!(&mut msg, "Panic: {payload}");

        let backtrace = std::backtrace::Backtrace::force_capture();
        let _ = writeln!(&mut msg, "\nBacktrace:\n{backtrace}");

        let data_dir = dirs::data_local_dir().map(|d| d.join("connected"));
        let crash_path = data_dir.as_ref().map(|d| {
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            d.join(format!("crash-{ts}.log"))
        });

        if let (Some(dir), Some(path)) = (data_dir, crash_path.clone()) {
            let _ = std::fs::create_dir_all(&dir);
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .open(&path)
            {
                let _ = f.write_all(msg.as_bytes());
            }
        }

        let mut dialog = String::new();
        let _ = writeln!(
            &mut dialog,
            "Connected crashed during startup.\n\nPanic: {payload}\n"
        );
        if let Some(path) = crash_path {
            let _ = writeln!(
                &mut dialog,
                "A crash report was written to:\n{}\n",
                path.display()
            );
        }
        let _ = writeln!(
            &mut dialog,
            "If this keeps happening after reboot, please include the crash report file."
        );

        let _ = rfd::MessageDialog::new()
            .set_title("Connected")
            .set_description(&dialog)
            .set_level(rfd::MessageLevel::Error)
            .show();
    }));
}

fn main() {
    #[cfg(target_os = "windows")]
    install_windows_panic_hook();

    // Explicitly select the Rustls crypto provider to avoid runtime ambiguity.
    if let Err(err) = rustls::crypto::ring::default_provider().install_default() {
        eprintln!("Failed to install rustls ring provider: {err:?}");
    }

    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    #[cfg(target_os = "windows")]
    {
        ensure_webview2_runtime_available();
        ensure_firewall_rules();
    }

    // Platform-specific window settings
    let decorations = cfg!(target_os = "windows");
    let transparent = !cfg!(target_os = "windows");

    let data_dir = dirs::data_local_dir().map(|d| d.join("connected"));
    if let Some(d) = data_dir.as_ref().filter(|d| !d.exists()) {
        let _ = std::fs::create_dir_all(d);
    }

    #[allow(unused_mut)]
    let mut config = dioxus::desktop::Config::new()
        .with_window(
            dioxus::desktop::WindowBuilder::new()
                .with_title("Connected")
                .with_inner_size(dioxus::desktop::LogicalSize::new(1100.0, 700.0))
                .with_decorations(decorations)
                .with_transparent(transparent)
                .with_window_icon(Some(load_icon())),
        )
        .with_menu(None)
        .with_disable_context_menu(true);

    if let Some(d) = data_dir {
        config = config.with_data_directory(d);
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        config = config.with_close_behaviour(dioxus::desktop::WindowCloseBehaviour::WindowHides);
    }

    LaunchBuilder::desktop().with_cfg(config).launch(App);
}

fn App() -> Element {
    // UI State
    let initialized = use_signal(|| false);
    let mut local_device_name =
        use_signal(|| get_device_name_setting().unwrap_or_else(get_hostname));
    let local_device_ip = use_signal(String::new);
    let mut devices_list = use_signal(Vec::<DeviceInfo>::new);
    let mut selected_device = use_signal(|| None::<DeviceInfo>);
    let mut active_tab = use_signal(|| "devices".to_string());
    let mut device_detail_tab = use_signal(|| "clipboard".to_string());
    let mut transfer_status = use_signal(|| TransferStatus::Idle);
    let mut clipboard_sync_enabled = use_signal(get_clipboard_sync_enabled);
    let mut notifications = use_signal(Vec::<Notification>::new);
    let mut show_send_dialog = use_signal(|| false);
    let mut send_dialog_is_folder = use_signal(|| false);
    let mut send_target_device = use_signal(|| None::<DeviceInfo>);
    let mut clipboard_text = use_signal(String::new);
    let mut show_rename_dialog = use_signal(|| false);
    let mut rename_text = use_signal(String::new);

    let discovery_active = use_signal(|| false);
    let mut media_enabled = use_signal(get_media_enabled_setting);
    let mut current_media_title = use_signal(|| "Not Playing".to_string());
    let mut current_media_artist = use_signal(String::new);
    let mut current_media_playing = use_signal(|| false);
    let mut current_media_source_id = use_signal(|| "local".to_string());

    // Pairing State
    let mut pairing_mode = use_signal(|| *get_pairing_mode_state().lock().unwrap());
    let mut pairing_requests = use_signal(Vec::<PairingRequest>::new);
    let mut file_transfer_requests = use_signal(Vec::<FileTransferRequest>::new);

    // Telephony State
    let mut phone_sub_tab = use_signal(|| "messages".to_string());
    let mut phone_contacts = use_signal(Vec::<connected_core::telephony::Contact>::new);
    let mut phone_conversations = use_signal(Vec::<connected_core::telephony::Conversation>::new);
    let mut phone_call_log = use_signal(Vec::<connected_core::telephony::CallLogEntry>::new);
    let mut selected_conversation = use_signal(|| None::<String>);
    let mut phone_messages = use_signal(Vec::<connected_core::telephony::SmsMessage>::new);
    let mut last_message_count = use_signal(|| 0usize);
    let mut sms_compose_text = use_signal(String::new);
    let mut active_call = use_signal(|| None::<connected_core::telephony::ActiveCall>);
    let mut update_info = use_signal(|| None::<UpdateInfo>);

    // Auto-sync settings (loaded from persistent storage)
    let mut auto_sync_messages = use_signal(get_auto_sync_messages);
    let mut auto_sync_calls = use_signal(get_auto_sync_calls);
    let mut auto_sync_contacts = use_signal(get_auto_sync_contacts);
    let mut notifications_enabled = use_signal(get_notifications_enabled_setting);

    // The Controller
    let action_tx =
        use_coroutine(
            move |rx: UnboundedReceiver<AppAction>| async move { app_controller(rx).await },
        );
    let (mpris_tx, mpris_rx) = std::sync::mpsc::channel();
    if mpris_server::init_mpris(mpris_tx) {
        spawn(async move {
            loop {
                while let Ok(command) = mpris_rx.try_recv() {
                    action_tx.send(AppAction::ControlRemoteMedia(command));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
    }

    // Start Init and initialize settings-based features
    use_effect(move || {
        action_tx.send(AppAction::Init);

        // Start media control if it was enabled in saved settings
        if get_media_enabled_setting() {
            action_tx.send(AppAction::ToggleMediaControl {
                enabled: true,
                notify: false,
            });
        }

        // Poll for Bluetooth/Wi-Fi state changes
        let action_tx_clone = action_tx;
        spawn(async move {
            // Helper to check states
            async fn check_adapters() -> (bool, bool) {
                #[cfg(target_os = "linux")]
                {
                    let bt_status = tokio::process::Command::new("rfkill")
                        .arg("list")
                        .arg("bluetooth")
                        .output()
                        .await;
                    let bt_on = if let Ok(output) = bt_status {
                        let out = String::from_utf8_lossy(&output.stdout);
                        !out.contains("Soft blocked: yes") && !out.contains("Hard blocked: yes")
                    } else {
                        true
                    };

                    let wifi_status = tokio::process::Command::new("nmcli")
                        .arg("radio")
                        .arg("wifi")
                        .output()
                        .await;
                    let wifi_on = if let Ok(output) = wifi_status {
                        let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
                        out == "enabled"
                    } else {
                        true
                    };
                    (bt_on, wifi_on)
                }
                #[cfg(not(target_os = "linux"))]
                {
                    (true, true)
                }
            }

            // Init state
            let (mut last_bt_state, mut last_wifi_state) = check_adapters().await;

            loop {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;

                let (bt_on, wifi_on) = check_adapters().await;

                let changed = bt_on != last_bt_state || wifi_on != last_wifi_state;

                if changed {
                    debug!(
                        "Adapter state changed (BT: {} -> {}, WiFi: {} -> {})",
                        last_bt_state, bt_on, last_wifi_state, wifi_on
                    );
                    action_tx_clone.send(AppAction::RefreshDiscovery);
                }

                last_bt_state = bt_on;
                last_wifi_state = wifi_on;
            }
        });
    });

    #[cfg(target_os = "windows")]
    {
        use dioxus::desktop::trayicon::menu::{Menu, MenuItem, PredefinedMenuItem};

        let window = use_window();
        let window = window.window.clone();

        let (show_id, hide_id, quit_id) = use_hook(|| {
            let menu = Menu::new();
            let show = MenuItem::new("Show Connected", true, None);
            let hide = MenuItem::new("Hide Connected", true, None);
            let quit = MenuItem::new("Quit", true, None);

            menu.append_items(&[&show, &hide, &PredefinedMenuItem::separator(), &quit])
                .expect("Failed to build tray menu");

            dioxus::desktop::trayicon::init_tray_icon(menu, Some(load_tray_icon()));

            (show.id().clone(), hide.id().clone(), quit.id().clone())
        });

        // NOTE: Dioxus installs a global `muda::MenuEvent` handler. Tray menu clicks are delivered
        // through that same event stream on Windows, so `use_muda_event_handler` is the reliable hook.
        dioxus::desktop::use_muda_event_handler(move |event| {
            if event.id == show_id {
                window.set_visible(true);
                window.set_minimized(false);
                window.set_focus();
            } else if event.id == hide_id {
                window.set_visible(false);
            } else if event.id == quit_id {
                std::process::exit(0);
            }
        });
    }

    #[cfg(target_os = "linux")]
    {
        use ksni::TrayMethods;

        let window = use_window();

        // Initialize tray icon
        use_hook(|| {
            let tray = tray::ConnectedTray {
                window: window.window.clone(),
            };
            // Spawn the tray service using the ksni async API
            tokio::spawn(async move {
                match tray.spawn().await {
                    Ok(_handle) => {
                        debug!("System tray initialized");
                    }
                    Err(e) => {
                        tracing::error!("Failed to initialize system tray: {:?}", e);
                    }
                }
            });
        });
    }

    // Auto-sync phone data when a device is selected
    use_effect(move || {
        if let Some(ref dev) = *selected_device.read() {
            set_phone_data_device(Some(dev.id.clone()));
            let dev_ip = dev.ip.clone();
            let dev_port = dev.port;

            if *auto_sync_messages.read() && !is_messages_synced() {
                action_tx.send(AppAction::RequestConversationsSync {
                    ip: dev_ip.clone(),
                    port: dev_port,
                });
            }
            if *auto_sync_calls.read() && !is_calls_synced() {
                action_tx.send(AppAction::RequestCallLog {
                    ip: dev_ip.clone(),
                    port: dev_port,
                    limit: 200,
                });
            }
            if *auto_sync_contacts.read() && !is_contacts_synced() {
                action_tx.send(AppAction::RequestContactsSync {
                    ip: dev_ip,
                    port: dev_port,
                });
            }
        } else {
            set_phone_data_device(None);
        }
    });

    // UI Poller
    use_future(move || async move {
        loop {
            // Update devices list
            let mut list: Vec<DeviceInfo> = get_devices_store()
                .lock()
                .unwrap()
                .values()
                .cloned()
                .collect();

            // Apply pending state
            {
                let pending = get_pending_pairings().lock().unwrap();
                for device in list.iter_mut() {
                    if pending.contains(&device.id) {
                        device.is_pending = true;
                    }
                }
            }

            devices_list.set(list);
            // Update transfer status
            let status = get_transfer_status().lock().unwrap().clone();
            transfer_status.set(status);

            // Update notifications
            {
                let mut notifs = get_notifications().lock().unwrap();
                let now = std::time::Instant::now();
                notifs.retain(|n| now.duration_since(n.timestamp).as_secs() < 5);
            }
            notifications.set(get_notifications().lock().unwrap().clone());

            // Update Pairing Requests
            {
                let reqs = get_pairing_requests().lock().unwrap().clone();
                pairing_requests.set(reqs);
            }

            // Update File Transfer Requests
            {
                let reqs_map = get_file_transfer_requests().lock().unwrap();
                let mut reqs: Vec<FileTransferRequest> = reqs_map.values().cloned().collect();
                reqs.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
                file_transfer_requests.set(reqs);
            }

            // Update Telephony State
            phone_contacts.set(get_phone_contacts().lock().unwrap().clone());
            phone_conversations.set(get_phone_conversations().lock().unwrap().clone());
            phone_call_log.set(get_phone_call_log().lock().unwrap().clone());
            active_call.set(get_active_call().lock().unwrap().clone());
            // Update messages for selected conversation
            if let Some(thread_id) = selected_conversation.read().clone()
                && let Some(msgs) = get_phone_messages().lock().unwrap().get(&thread_id)
            {
                let new_count = msgs.len();
                let old_count = *last_message_count.read();
                phone_messages.set(msgs.clone());
                // Auto-scroll when messages first load or when new messages arrive
                if new_count > 0 && new_count != old_count {
                    spawn(async move {
                        // Delay to let DOM update
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        let js = r#"
                                let el = document.getElementById('messages-container');
                                if (el) { el.scrollTop = el.scrollHeight; }
                            "#;
                        let _ = document::eval(js);
                    });
                }
                last_message_count.set(new_count);
            }

            // Update Media State
            media_enabled.set(*get_media_enabled().lock().unwrap());
            if let Some(media) = get_current_media().lock().unwrap().clone() {
                current_media_title.set(
                    media
                        .state
                        .title
                        .unwrap_or_else(|| "Unknown Title".to_string()),
                );
                current_media_artist.set(
                    media
                        .state
                        .artist
                        .unwrap_or_else(|| "Unknown Artist".to_string()),
                );
                current_media_playing.set(media.state.playing);
                current_media_source_id.set(media.source_device_id);
            } else {
                current_media_title.set("Not Playing".to_string());
                current_media_artist.set(String::new());
                current_media_playing.set(false);
                current_media_source_id.set("local".to_string());
            }

            pairing_mode.set(*get_pairing_mode_state().lock().unwrap());

            // Update Info
            {
                let info = get_update_info().lock().unwrap().clone();
                update_info.set(info);
            }

            // Clipboard Sync Check
            if *clipboard_sync_enabled.read() {
                // Check if we recently received a remote update (debounce to prevent echo)
                let last_update = *get_last_remote_update().lock().unwrap();
                if last_update.elapsed() >= Duration::from_millis(1000) {
                    let current_clip = get_system_clipboard();
                    let last_clip = get_last_clipboard().lock().unwrap().clone();
                    if !current_clip.is_empty() && current_clip != last_clip {
                        debug!(
                            "Local clipboard changed. New content length: {}",
                            current_clip.len()
                        );
                        *get_last_clipboard().lock().unwrap() = current_clip.clone();
                        action_tx.send(AppAction::BroadcastClipboard { text: current_clip });
                    }
                }
            }

            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    });

    let toggle_clipboard_sync = move |_| {
        let current = *clipboard_sync_enabled.read();
        let new_state = !current;
        clipboard_sync_enabled.set(new_state);
        set_clipboard_sync_enabled(new_state); // Save to disk
        action_tx.send(AppAction::SetClipboardSync(new_state));
        if new_state {
            *get_last_clipboard().lock().unwrap() = get_system_clipboard();
            add_notification("Clipboard Sync", "Sync Started (Trusted Devices)", "");
        } else {
            add_notification("Clipboard Sync", "Sync Stopped", "");
        }
    };

    let toggle_pairing_mode = move |_| {
        let current = *pairing_mode.read();
        action_tx.send(AppAction::SetPairingMode(!current));
    };

    let toggle_media = move |_| {
        let current = *media_enabled.read();
        let new_state = !current;
        set_media_enabled_setting(new_state); // Save to disk
        action_tx.send(AppAction::ToggleMediaControl {
            enabled: new_state,
            notify: true,
        });
    };

    rsx! {
        style { {include_str!("../assets/styles.css")} }

        div {
            class: "app-container",

            // Sidebar
            aside {
                class: "sidebar",

                // Logo/Header
                div {
                    class: "sidebar-header",
                    Icon { icon: IconType::Logo, size: 36, color: "white".to_string() }
                    h1 { "Connected" }
                }

                // Local device info
                div {
                    class: "local-device",
                    div {
                        class: "local-device-info",
                        span { class: "local-device-name", "{local_device_name}" }
                        span { class: "local-device-ip", "{local_device_ip}" }
                    }
                    div {
                        class: if *initialized.read() { "status-dot online" } else { "status-dot" }
                    }
                }

                // Navigation
                nav {
                    class: "sidebar-nav",
                    button {
                        class: if *active_tab.read() == "devices" && selected_device.read().is_none() { "nav-item active" } else { "nav-item" },
                        onclick: move |_| {
                            active_tab.set("devices".to_string());
                            selected_device.set(None);
                        },
                        Icon { icon: IconType::NavDevices, size: 18, color: "currentColor".to_string() }
                        span { "Devices" }
                        if !devices_list.read().is_empty() {
                            span { class: "nav-badge", "{devices_list.read().len()}" }
                        }
                    }

                    // Show device feature tabs when a device is selected
                    if selected_device.read().is_some() {
                        div { class: "nav-divider" }
                        if let Some(ref dev) = *selected_device.read() {
                            div {
                                class: "nav-device-name",
                                "{dev.name}"
                            }
                        }
                        button {
                            class: if *active_tab.read() == "devices" && *device_detail_tab.read() == "clipboard" { "nav-item active" } else { "nav-item" },
                            onclick: move |_| {
                                active_tab.set("devices".to_string());
                                device_detail_tab.set("clipboard".to_string());
                            },
                            Icon { icon: IconType::NavClipboard, size: 18, color: "currentColor".to_string() }
                            span { "Clipboard" }
                        }
                        button {
                            class: if *active_tab.read() == "devices" && *device_detail_tab.read() == "transfers" { "nav-item active" } else { "nav-item" },
                            onclick: move |_| {
                                active_tab.set("devices".to_string());
                                device_detail_tab.set("transfers".to_string());
                            },
                            Icon { icon: IconType::NavTransfers, size: 18, color: "currentColor".to_string() }
                            span { "Transfers" }
                        }
                        button {
                            class: if *active_tab.read() == "devices" && *device_detail_tab.read() == "files" { "nav-item active" } else { "nav-item" },
                            onclick: move |_| {
                                active_tab.set("devices".to_string());
                                device_detail_tab.set("files".to_string());
                            },
                            Icon { icon: IconType::NavFiles, size: 18, color: "currentColor".to_string() }
                            span { "Files" }
                        }
                        button {
                            class: if *active_tab.read() == "devices" && *device_detail_tab.read() == "media" { "nav-item active" } else { "nav-item" },
                            onclick: move |_| {
                                active_tab.set("devices".to_string());
                                device_detail_tab.set("media".to_string());
                            },
                            Icon { icon: IconType::NavMedia, size: 18, color: "currentColor".to_string() }
                            span { "Media" }
                        }
                        button {
                            class: if *active_tab.read() == "devices" && *device_detail_tab.read() == "phone" { "nav-item active" } else { "nav-item" },
                            onclick: move |_| {
                                active_tab.set("devices".to_string());
                                device_detail_tab.set("phone".to_string());
                                // Trigger auto-sync for the current phone sub-tab if enabled and data is empty
                                if let Some(ref dev) = *selected_device.read() {
                                    let current_sub = phone_sub_tab.read().clone();
                                    if current_sub == "messages" && *auto_sync_messages.read() && phone_conversations.read().is_empty() {
                                        action_tx.send(AppAction::RequestConversationsSync {
                                            ip: dev.ip.clone(),
                                            port: dev.port,
                                        });
                                    } else if current_sub == "calls" && *auto_sync_calls.read() && phone_call_log.read().is_empty() {
                                        action_tx.send(AppAction::RequestCallLog {
                                            ip: dev.ip.clone(),
                                            port: dev.port,
                                            limit: 200,
                                        });
                                    } else if current_sub == "contacts" && *auto_sync_contacts.read() && phone_contacts.read().is_empty() {
                                        action_tx.send(AppAction::RequestContactsSync {
                                            ip: dev.ip.clone(),
                                            port: dev.port,
                                        });
                                    }
                                }
                            },
                            Icon { icon: IconType::NavPhone, size: 18, color: "currentColor".to_string() }
                            span { "Phone" }
                        }
                        div { class: "nav-divider" }
                    }

                    button {
                        class: if *active_tab.read() == "settings" { "nav-item active" } else { "nav-item" },
                        onclick: move |_| active_tab.set("settings".to_string()),
                        Icon { icon: IconType::NavSettings, size: 18, color: "currentColor".to_string() }
                        span { "Settings" }
                    }
                }

                // Status indicator
                div {
                    class: "sidebar-footer",
                    div {
                        class: "discovery-status",
                        if *initialized.read() && *discovery_active.read() {
                            span { "Discovering" }
                        } else {
                            span { "Starting..." }
                        }
                    }
                }
            }

            // Main content
            main {
                class: "main-content",

                // Tab: Devices
                if *active_tab.read() == "devices" && selected_device.read().is_none() {
                    div {
                        class: "content-header",
                        h2 { "Nearby Devices" }
                        span { class: "content-subtitle", "{devices_list.read().len()} device(s) found" }
                    }

                    if devices_list.read().is_empty() {
                        div {
                            class: "empty-state",
                            div {
                                class: "empty-icon searching",
                                Icon { icon: IconType::Searching, size: 64, color: "var(--text-tertiary)".to_string() }
                            }
                            h3 { "Looking for Devices" }
                            p { "Make sure other devices are on the same network." }
                            div { class: "searching-indicator",
                                span { class: "dot" }
                                span { class: "dot" }
                                span { class: "dot" }
                            }
                        }
                    } else {
                        div {
                            class: "device-grid",
                            for device in devices_list.read().iter() {
                                DeviceCard {
                                    key: "{device.id}",
                                    device: device.clone(),
                                    is_selected: false,
                                    on_select: move |d: DeviceInfo| {
                                        selected_device.set(Some(d));
                                        device_detail_tab.set("clipboard".to_string());
                                    },
                                    on_pair: move |d: DeviceInfo| {
                                         if let Ok(port) = d.port.to_string().parse() {
                                             action_tx.send(AppAction::PairWithDevice { ip: d.ip.clone(), port });
                                         }
                                    },
                                    on_send_file: move |d: DeviceInfo| {
                                        send_target_device.set(Some(d));
                                        show_send_dialog.set(true);
                                    },
                                }
                            }
                        }
                    }
                }

                // Device Detail View - shows when a device is selected
                if *active_tab.read() == "devices" && selected_device.read().is_some() {
                    if let Some(device) = selected_device.read().as_ref() {
                        div {
                            class: "device-detail-view",

                            // Compact header with device info and actions
                            div {
                                class: "device-detail-header",
                                button {
                                    class: "back-button",
                                    onclick: move |_| {
                                        selected_device.set(None);
                                        active_tab.set("devices".to_string());
                                    },
                                    Icon { icon: IconType::Back, size: 14, color: "currentColor".to_string() }
                                }
                                div {
                                    class: "device-detail-info",
                                    span { class: "device-address", "{device.ip}:{device.port}" }
                                    span { class: "device-type-badge", "{device.device_type}" }
                                }
                                div {
                                    class: "device-detail-actions",
                                    button {
                                        class: "header-action-btn",
                                        title: "Unpair device",
                                        onclick: {
                                            let device = device.clone();
                                            move |_| {
                                                action_tx.send(AppAction::UnpairDevice { device_id: device.id.clone() });
                                                selected_device.set(None);
                                            }
                                        },
                                        Icon { icon: IconType::Unpair, size: 14, color: "currentColor".to_string() }
                                    }
                                    button {
                                        class: "header-action-btn warning",
                                        title: "Forget device",
                                        onclick: {
                                            let device = device.clone();
                                            move |_| {
                                                action_tx.send(AppAction::ForgetDevice { device_id: device.id.clone() });
                                                selected_device.set(None);
                                            }
                                        },
                                        Icon { icon: IconType::Refresh, size: 14, color: "currentColor".to_string() }
                                    }
                                }
                            }

                            // Full-width content area (no secondary sidebar)
                            div {
                                class: "device-detail-content",

                                // Clipboard sub-tab
                                if *device_detail_tab.read() == "clipboard" {
                                    div {
                                        class: "clipboard-section",
                                        div {
                                            class: if *clipboard_sync_enabled.read() { "sync-status active" } else { "sync-status" },
                                            if *clipboard_sync_enabled.read() {
                                                div {
                                                    class: "sync-icon",
                                                    Icon { icon: IconType::Sync, size: 32, color: "var(--accent)".to_string() }
                                                }
                                            } else {
                                                div {
                                                    class: "sync-icon muted",
                                                    Icon { icon: IconType::NavClipboard, size: 32, color: "var(--text-tertiary)".to_string() }
                                                }
                                            }

                                            div {
                                                class: "sync-info",
                                                span {
                                                    class: if *clipboard_sync_enabled.read() { "sync-label" } else { "sync-label muted" },
                                                    "Universal Clipboard"
                                                }
                                                span { class: "sync-hint", "Sync with all trusted devices" }
                                            }

                                            label {
                                                class: "toggle-switch",
                                                input {
                                                    r#type: "checkbox",
                                                    checked: "{clipboard_sync_enabled}",
                                                    oninput: toggle_clipboard_sync,
                                                }
                                                span { class: "slider" }
                                            }
                                        }

                                        div {
                                            class: "manual-clipboard",
                                            h3 { "Send to {device.name}" }
                                            textarea {
                                                class: "clipboard-input",
                                                placeholder: "Enter text to send...",
                                                value: "{clipboard_text}",
                                                oninput: move |evt| clipboard_text.set(evt.value().clone()),
                                            }
                                            div {
                                                class: "clipboard-actions",
                                                button {
                                                    class: "secondary-button",
                                                    onclick: move |_| clipboard_text.set(get_system_clipboard()),
                                                    Icon { icon: IconType::Paste, size: 14, color: "currentColor".to_string() }
                                                    span { " Paste from Clipboard" }
                                                }
                                                button {
                                                    class: "primary-button",
                                                    disabled: clipboard_text.read().is_empty(),
                                                    onclick: {
                                                        let device = device.clone();
                                                        move |_| {
                                                            let ip = device.ip.clone();
                                                            let port = device.port;
                                                            let text = clipboard_text.read().clone();
                                                            if !text.is_empty() {
                                                                action_tx.send(AppAction::SendClipboard { ip, port, text });
                                                                add_notification("Clipboard", "Sending...", "send");
                                                            }
                                                        }
                                                    },
                                                    Icon { icon: IconType::Send, size: 14, color: "currentColor".to_string() }
                                                    span { " Send" }
                                                }
                                            }
                                        }
                                    }
                                }

                                // Transfers sub-tab
                                if *device_detail_tab.read() == "transfers" {
                                    div {
                                        class: "transfers-section",
                                        match &*transfer_status.read() {
                                            TransferStatus::Idle => rsx! {
                                                div {
                                                    class: "transfer-idle",
                                                    div {
                                                        class: "transfer-icon",
                                                        Icon { icon: IconType::Folder, size: 48, color: "var(--text-tertiary)".to_string() }
                                                    }
                                                    p { "No active transfers" }
                                                }
                                            },
                                            TransferStatus::Starting(filename) => rsx! {
                                                div {
                                                    class: "transfer-active",
                                                    div {
                                                        class: "transfer-icon spinning",
                                                        Icon { icon: IconType::Sync, size: 48, color: "var(--accent)".to_string() }
                                                    }
                                                    p { "Starting transfer: {filename}" }
                                                }
                                            },
                                            TransferStatus::InProgress { filename, percent } => rsx! {
                                                div {
                                                    class: "transfer-active",
                                                    div { class: "transfer-info",
                                                        span { class: "transfer-filename", "{filename}" }
                                                        span { class: "transfer-percent", "{percent:.1}%" }
                                                    }
                                                    div {
                                                        class: "progress-bar",
                                                        div {
                                                            class: "progress-fill",
                                                            style: "width: {percent}%",
                                                        }
                                                    }
                                                }
                                            },
                                            TransferStatus::Completed(filename) => rsx! {
                                                div {
                                                    class: "transfer-complete",
                                                    div {
                                                        class: "transfer-icon",
                                                        Icon { icon: IconType::Check, size: 48, color: "var(--success)".to_string() }
                                                    }
                                                    p { "{filename} received successfully!" }
                                                }
                                            },
                                            TransferStatus::Failed(error) => rsx! {
                                                div {
                                                    class: "transfer-failed",
                                                    div {
                                                        class: "transfer-icon",
                                                        Icon { icon: IconType::Error, size: 48, color: "var(--error)".to_string() }
                                                    }
                                                    p { "Transfer failed: {error}" }
                                                }
                                            },
                                        }

                                        div {
                                            class: "send-file-section",
                                            h3 { "Send a File or Folder to {device.name}" }
                                            div {
                                                style: "display: flex; gap: 10px;",
                                                button {
                                                    class: "primary-button",
                                                    onclick: {
                                                        let device = device.clone();
                                                        move |_| {
                                                            send_target_device.set(Some(device.clone()));
                                                            send_dialog_is_folder.set(false);
                                                            show_send_dialog.set(true);
                                                        }
                                                    },
                                                    Icon { icon: IconType::Upload, size: 14, color: "currentColor".to_string() }
                                                    span { " Choose File" }
                                                }
                                                button {
                                                    class: "primary-button",
                                                    onclick: {
                                                        let device = device.clone();
                                                        move |_| {
                                                            send_target_device.set(Some(device.clone()));
                                                            send_dialog_is_folder.set(true);
                                                            show_send_dialog.set(true);
                                                        }
                                                    },
                                                    Icon { icon: IconType::Folder, size: 14, color: "currentColor".to_string() }
                                                    span { " Choose Folder" }
                                                }
                                            }
                                        }

                                        div {
                                            class: "download-location",
                                            span { class: "label", "Save to:" }
                                            span {
                                                class: "path",
                                                {dirs::download_dir().unwrap_or_else(|| PathBuf::from(".")).display().to_string()}
                                            }
                                        }
                                    }
                                }

                                // Media sub-tab - Control Android device media from desktop
                                if *device_detail_tab.read() == "media" {
                                    div {
                                        class: "media-section",

                                        // Unified Android Media Control Card
                                        div {
                                            class: "media-controls",
                                            h3 {
                                                Icon { icon: IconType::Music, size: 20, color: "var(--accent)".to_string() }
                                                span { " Control {device.name}" }
                                            }

                                            // Show what's playing on the Android device
                                            div {
                                                class: "media-info",
                                                if current_media_title.read().as_str() == "Not Playing" || current_media_source_id.read().as_str() != device.id {
                                                    div { class: "muted", "No media playing on this device" }
                                                } else {
                                                    div {
                                                        div { class: "media-title", "{current_media_title}" }
                                                        div { class: "media-artist", "{current_media_artist}" }
                                                        div {
                                                            class: "media-status",
                                                            if *current_media_playing.read() {
                                                                Icon { icon: IconType::Play, size: 12, color: "var(--success)".to_string() }
                                                                span { " Playing" }
                                                            } else {
                                                                Icon { icon: IconType::Pause, size: 12, color: "var(--text-secondary)".to_string() }
                                                                span { " Paused" }
                                                            }
                                                        }
                                                    }
                                                }
                                            }

                                            // Playback Controls
                                            div {
                                                class: "control-buttons",
                                                button {
                                                    class: "control-btn",
                                                    title: "Previous",
                                                    onclick: {
                                                        let device = device.clone();
                                                        move |_| {
                                                            if let Ok(port) = device.port.to_string().parse() {
                                                                action_tx.send(AppAction::SendMediaCommand { ip: device.ip.clone(), port, command: MediaCommand::Previous });
                                                            }
                                                        }
                                                    },
                                                    Icon { icon: IconType::Previous, size: 24, color: "currentColor".to_string() }
                                                }
                                                button {
                                                    class: "control-btn",
                                                    title: "Play/Pause",
                                                    onclick: {
                                                        let device = device.clone();
                                                        move |_| {
                                                            if let Ok(port) = device.port.to_string().parse() {
                                                                action_tx.send(AppAction::SendMediaCommand { ip: device.ip.clone(), port, command: MediaCommand::PlayPause });
                                                            }
                                                        }
                                                    },
                                                    Icon { icon: IconType::Play, size: 24, color: "currentColor".to_string() }
                                                }
                                                button {
                                                    class: "control-btn",
                                                    title: "Next",
                                                    onclick: {
                                                        let device = device.clone();
                                                        move |_| {
                                                            if let Ok(port) = device.port.to_string().parse() {
                                                                action_tx.send(AppAction::SendMediaCommand { ip: device.ip.clone(), port, command: MediaCommand::Next });
                                                            }
                                                        }
                                                    },
                                                    Icon { icon: IconType::Next, size: 24, color: "currentColor".to_string() }
                                                }
                                            }

                                            // Volume Controls
                                            div {
                                                class: "control-buttons secondary",
                                                button {
                                                    class: "control-btn small",
                                                    title: "Volume Down",
                                                    onclick: {
                                                        let device = device.clone();
                                                        move |_| {
                                                            if let Ok(port) = device.port.to_string().parse() {
                                                                action_tx.send(AppAction::SendMediaCommand { ip: device.ip.clone(), port, command: MediaCommand::VolumeDown });
                                                            }
                                                        }
                                                    },
                                                    Icon { icon: IconType::VolumeDown, size: 20, color: "currentColor".to_string() }
                                                }
                                                button {
                                                    class: "control-btn small",
                                                    title: "Volume Up",
                                                    onclick: {
                                                        let device = device.clone();
                                                        move |_| {
                                                            if let Ok(port) = device.port.to_string().parse() {
                                                                action_tx.send(AppAction::SendMediaCommand { ip: device.ip.clone(), port, command: MediaCommand::VolumeUp });
                                                            }
                                                        }
                                                    },
                                                    Icon { icon: IconType::VolumeUp, size: 20, color: "currentColor".to_string() }
                                                }
                                            }
                                        }

                                        // Note about media control
                                        div {
                                            class: "phone-note",
                                            span { class: "note-icon", Icon { icon: IconType::Warning, size: 16, color: "var(--text-tertiary)".to_string() } }
                                            span { "Control media playback on your Android device. Make sure a media app is playing on the device." }
                                        }
                                    }
                                }

                                // Browse Files sub-tab
                                if *device_detail_tab.read() == "files" {
                                    FileBrowser {
                                        device: device.clone(),
                                        on_close: move |_| device_detail_tab.set("clipboard".to_string()),
                                    }
                                }

                                // Phone sub-tab (SMS, Calls, Contacts)
                                if *device_detail_tab.read() == "phone" {
                                    div {
                                        class: "phone-section",

                                        // Phone sub-navigation tabs
                                        div {
                                            class: "phone-tabs",
                                            button {
                                                class: if *phone_sub_tab.read() == "messages" { "phone-tab active" } else { "phone-tab" },
                                                onclick: {
                                                    let device_id = device.id.clone();
                                                    move |_| {
                                                        phone_sub_tab.set("messages".to_string());
                                                        // Only sync if auto-sync enabled AND data is empty
                                                        if *auto_sync_messages.read() && phone_conversations.read().is_empty()
                                                            && let Some(fresh_device) = get_devices_store().lock().unwrap().get(&device_id).cloned()
                                                        {
                                                            action_tx.send(AppAction::RequestConversationsSync {
                                                                ip: fresh_device.ip.clone(),
                                                                port: fresh_device.port,
                                                            });
                                                        }
                                                    }
                                                },
                                                Icon { icon: IconType::Message, size: 16, color: "currentColor".to_string() }
                                                span { " Messages" }
                                            }
                                            button {
                                                class: if *phone_sub_tab.read() == "calls" { "phone-tab active" } else { "phone-tab" },
                                                onclick: {
                                                    let device_id = device.id.clone();
                                                    move |_| {
                                                        phone_sub_tab.set("calls".to_string());
                                                        // Only sync if auto-sync enabled AND data is empty
                                                        if *auto_sync_calls.read() && phone_call_log.read().is_empty()
                                                            && let Some(fresh_device) = get_devices_store().lock().unwrap().get(&device_id).cloned()
                                                        {
                                                            action_tx.send(AppAction::RequestCallLog {
                                                                ip: fresh_device.ip.clone(),
                                                                port: fresh_device.port,
                                                                limit: 200,
                                                            });
                                                        }
                                                    }
                                                },
                                                Icon { icon: IconType::Call, size: 16, color: "currentColor".to_string() }
                                                span { " Calls" }
                                            }
                                            button {
                                                class: if *phone_sub_tab.read() == "contacts" { "phone-tab active" } else { "phone-tab" },
                                                onclick: {
                                                    let device_id = device.id.clone();
                                                    move |_| {
                                                        phone_sub_tab.set("contacts".to_string());
                                                        // Only sync if auto-sync enabled AND data is empty
                                                        if *auto_sync_contacts.read() && phone_contacts.read().is_empty()
                                                            && let Some(fresh_device) = get_devices_store().lock().unwrap().get(&device_id).cloned()
                                                        {
                                                            action_tx.send(AppAction::RequestContactsSync {
                                                                ip: fresh_device.ip.clone(),
                                                                port: fresh_device.port,
                                                            });
                                                        }
                                                    }
                                                },
                                                Icon { icon: IconType::Contact, size: 16, color: "currentColor".to_string() }
                                                span { " Contacts" }
                                            }
                                        }

                                        // Messages Tab Content
                                        if *phone_sub_tab.read() == "messages" {
                                            if let Some(ref thread_id) = *selected_conversation.read() {
                                                // Thread view - show individual messages
                                                {
                                                    let convo_info = phone_conversations.read().iter()
                                                        .find(|c| c.id == *thread_id)
                                                        .cloned();
                                                    let contact_name = convo_info.as_ref()
                                                        .and_then(|c| c.contact_names.first().cloned())
                                                        .or_else(|| convo_info.as_ref().and_then(|c| c.addresses.first().cloned()))
                                                        .unwrap_or_else(|| "Unknown".to_string());
                                                    let recipient_address = convo_info.as_ref()
                                                        .and_then(|c| c.addresses.first().cloned())
                                                        .unwrap_or_default();
                                                    let first_char = contact_name.chars().next().unwrap_or('?');

                                                    rsx! {
                                                        div {
                                                            class: "phone-content message-thread-view",

                                                            // Thread header with back button
                                                            div {
                                                                class: "thread-header",
                                                                button {
                                                                    class: "back-button",
                                                                    onclick: move |_| {
                                                                        selected_conversation.set(None);
                                                                        sms_compose_text.set(String::new());
                                                                        last_message_count.set(0);
                                                                        phone_messages.set(Vec::new());
                                                                    },
                                                                    Icon { icon: IconType::Back, size: 14, color: "currentColor".to_string() }
                                                                    span { " Back" }
                                                                }
                                                                div {
                                                                    class: "thread-contact",
                                                                    div {
                                                                        class: "thread-avatar",
                                                                        "{first_char}"
                                                                    }
                                                                    div {
                                                                        class: "thread-contact-info",
                                                                        span { class: "thread-contact-name", "{contact_name}" }
                                                                        span { class: "thread-contact-number", "{recipient_address}" }
                                                                    }
                                                                }
                                                            }

                                                            // Messages list
                                                            div {
                                                                id: "messages-container",
                                                                class: "messages-container",
                                                                if phone_messages.read().is_empty() {
                                                                    div {
                                                                        class: "empty-state",
                                                                        span { class: "empty-icon", Icon { icon: IconType::Message, size: 48, color: "var(--text-tertiary)".to_string() } }
                                                                        p { "Loading messages..." }
                                                                    }
                                                                } else {
                                                                    div {
                                                                        class: "messages-list",
                                                                        for msg in phone_messages.read().iter() {
                                                                            {
                                                                                let is_outgoing = msg.is_outgoing;
                                                                                let body = msg.body.clone();
                                                                                let time_str = format_timestamp(msg.timestamp);
                                                                                let status_icon = match msg.status {
                                                                                    connected_core::telephony::SmsStatus::Pending => Some(IconType::Searching),
                                                                                    connected_core::telephony::SmsStatus::Sent => Some(IconType::Check),
                                                                                    connected_core::telephony::SmsStatus::Delivered => Some(IconType::Check),
                                                                                    connected_core::telephony::SmsStatus::Failed => Some(IconType::Error),
                                                                                    connected_core::telephony::SmsStatus::Received => None,
                                                                                };
                                                                                rsx! {
                                                                                    div {
                                                                                        class: if is_outgoing { "message-bubble outgoing" } else { "message-bubble incoming" },
                                                                                        div { class: "message-body", "{body}" }
                                                                                        div {
                                                                                            class: "message-meta",
                                                                                            span { class: "message-time", "{time_str}" }
                                                                                            if is_outgoing {
                                                                                                if let Some(icon) = status_icon {
                                                                                                    span { class: "message-status", Icon { icon: icon, size: 12, color: "currentColor".to_string() } }
                                                                                                }
                                                                                            }
                                                                                        }
                                                                                    }
                                                                                }
                                                                            }
                                                                        }
                                                                    }
                                                                }
                                                            }

                                                            // Compose area
                                                            div {
                                                                class: "compose-area",
                                                                input {
                                                                    r#type: "text",
                                                                    class: "compose-input",
                                                                    placeholder: "Type a message...",
                                                                    value: "{sms_compose_text.read()}",
                                                                    oninput: move |e| sms_compose_text.set(e.value().clone())
                                                                }
                                                                button {
                                                                    class: "send-button",
                                                                    disabled: sms_compose_text.read().is_empty(),
                                                                    onclick: {
                                                                        let device_id = device.id.clone();
                                                                        let to_address = recipient_address.clone();
                                                                        move |_| {
                                                                            let text = sms_compose_text.read().clone();
                                                                            if !text.is_empty() && !to_address.is_empty()
                                                                                && let Some(fresh_device) = get_devices_store().lock().unwrap().get(&device_id).cloned()
                                                                            {
                                                                                action_tx.send(AppAction::SendSms {
                                                                                    ip: fresh_device.ip.clone(),
                                                                                    port: fresh_device.port,
                                                                                    to: to_address.clone(),
                                                                                    body: text,
                                                                                });
                                                                                sms_compose_text.set(String::new());
                                                                            }
                                                                        }
                                                                    },
                                                                    "Send"
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            } else {
                                                // Conversation list view
                                                div {
                                                    class: "phone-content",
                                                    div {
                                                        class: "phone-header",
                                                        h4 { "Messages" }
                                                        if !*auto_sync_messages.read() {
                                                            button {
                                                                class: "sync-button",
                                                                onclick: {
                                                                    let device_id = device.id.clone();
                                                                    move |_| {
                                                                        if let Some(fresh_device) = get_devices_store().lock().unwrap().get(&device_id).cloned() {
                                                                            action_tx.send(AppAction::RequestConversationsSync {
                                                                                ip: fresh_device.ip.clone(),
                                                                                port: fresh_device.port,
                                                                            });
                                                                        } else {
                                                                            add_notification("Phone", "Device not found. Try refreshing.", "");
                                                                        }
                                                                    }
                                                                },
                                                                Icon { icon: IconType::Refresh, size: 14, color: "currentColor".to_string() }
                                                                span { " Sync" }
                                                            }
                                                        }
                                                    }

                                                    if phone_conversations.read().is_empty() {
                                                            div {
                                                                class: "empty-state",
                                                                span { class: "empty-icon", Icon { icon: IconType::Message, size: 48, color: "var(--text-tertiary)".to_string() } }
                                                                p { "No messages synced" }
                                                                if *auto_sync_messages.read() {
                                                                    p { class: "empty-hint", "Messages will sync automatically" }
                                                                } else {
                                                                    p { class: "empty-hint", "Click Sync to load your messages" }
                                                                }
                                                            }
                                                    } else {
                                                        div {
                                                            class: "conversation-list",
                                                            for convo in phone_conversations.read().iter() {
                                                                {
                                                                    let convo_id = convo.id.clone();
                                                                    let contact_name = convo.contact_names.first()
                                                                        .cloned()
                                                                        .unwrap_or_else(|| convo.addresses.first().cloned().unwrap_or_default());
                                                                    let last_msg = convo.last_message.clone().unwrap_or_default();
                                                                    let unread = convo.unread_count;
                                                                    let time_str = format_timestamp(convo.last_timestamp);

                                                                    rsx! {
                                                                        div {
                                                                            class: if unread > 0 { "conversation-item unread" } else { "conversation-item" },
                                                                            onclick: {
                                                                                let cid = convo_id.clone();
                                                                                let device_id = device.id.clone();
                                                                                move |_| {
                                                                                    selected_conversation.set(Some(cid.clone()));
                                                                                    last_message_count.set(0); // Reset to trigger scroll on load
                                                                                    phone_messages.set(Vec::new()); // Clear old messages
                                                                                    if let Some(fresh_device) = get_devices_store().lock().unwrap().get(&device_id).cloned() {
                                                                                        action_tx.send(AppAction::RequestMessages {
                                                                                            ip: fresh_device.ip.clone(),
                                                                                            port: fresh_device.port,
                                                                                            thread_id: cid.clone(),
                                                                                            limit: 200,
                                                                                        });
                                                                                    }
                                                                                }
                                                                            },
                                                                            div {
                                                                                class: "conversation-avatar",
                                                                                "{contact_name.chars().next().unwrap_or('?')}"
                                                                            }
                                                                            div {
                                                                                class: "conversation-info",
                                                                                div {
                                                                                    class: "conversation-header",
                                                                                    span { class: "conversation-name", "{contact_name}" }
                                                                                    span { class: "conversation-time", "{time_str}" }
                                                                                }
                                                                                div {
                                                                                    class: "conversation-preview",
                                                                                    "{last_msg}"
                                                                                }
                                                                            }
                                                                            if unread > 0 {
                                                                                span { class: "unread-badge", "{unread}" }
                                                                            }
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }

                                        // Calls Tab Content
                                        if *phone_sub_tab.read() == "calls" {
                                            div {
                                                class: "phone-content",
                                                div {
                                                    class: "phone-header",
                                                    h4 { "Call Log" }
                                                    if !*auto_sync_calls.read() {
                                                        button {
                                                            class: "sync-button",
                                                            onclick: {
                                                                let device_id = device.id.clone();
                                                                move |_| {
                                                                    if let Some(fresh_device) = get_devices_store().lock().unwrap().get(&device_id).cloned() {
                                                                        action_tx.send(AppAction::RequestCallLog {
                                                                            ip: fresh_device.ip.clone(),
                                                                            port: fresh_device.port,
                                                                            limit: 200,
                                                                        });
                                                                    } else {
                                                                        add_notification("Phone", "Device not found. Try refreshing.", "");
                                                                    }
                                                                }
                                                            },
                                                            Icon { icon: IconType::Refresh, size: 14, color: "currentColor".to_string() }
                                                            span { " Sync" }
                                                        }
                                                    }
                                                }

                                                if phone_call_log.read().is_empty() {
                                                    div {
                                                        class: "empty-state",
                                                        span { class: "empty-icon", Icon { icon: IconType::Call, size: 48, color: "var(--text-tertiary)".to_string() } }
                                                        p { "No call history synced" }
                                                        if *auto_sync_calls.read() {
                                                            p { class: "empty-hint", "Call log will sync automatically" }
                                                        } else {
                                                            p { class: "empty-hint", "Click Sync to load your call log" }
                                                        }
                                                    }
                                                } else {
                                                    div {
                                                        class: "call-log-list",
                                                        for call in phone_call_log.read().iter() {
                                                            {
                                                                let contact = call.contact_name.clone().unwrap_or_else(|| call.number.clone());
                                                                let call_type = &call.call_type;
                                                                let duration = call.duration;
                                                                let timestamp = call.timestamp;
                                                                let time_str = format_timestamp(timestamp);
                                                                let (icon_type, type_class) = match call_type {
                                                                    connected_core::telephony::CallType::Incoming => (IconType::Download, "incoming"),
                                                                    connected_core::telephony::CallType::Outgoing => (IconType::Upload, "outgoing"),
                                                                    connected_core::telephony::CallType::Missed => (IconType::Error, "missed"),
                                                                    connected_core::telephony::CallType::Rejected => (IconType::Warning, "rejected"),
                                                                    _ => (IconType::Call, "other"),
                                                                };
                                                                let duration_str = if duration > 0 {
                                                                    format!("{}:{:02}", duration / 60, duration % 60)
                                                                } else {
                                                                    "—".to_string()
                                                                };

                                                                rsx! {
                                                                    div {
                                                                        class: "call-item {type_class}",
                                                                        div { class: "call-icon", Icon { icon: icon_type, size: 16, color: "currentColor".to_string() } }
                                                                        div {
                                                                            class: "call-info",
                                                                            div {
                                                                                class: "call-contact",
                                                                                "{contact}"
                                                                            }
                                                                            div {
                                                                                class: "call-meta",
                                                                                span { class: "call-time", "{time_str}" }
                                                                                span { class: "call-duration", "{duration_str}" }
                                                                            }
                                                                        }
                                                                        button {
                                                                            class: "call-action",
                                                                            onclick: {
                                                                                let number = call.number.clone();
                                                                                let device_id = device.id.clone();
                                                                                move |_| {
                                                                                    if let Some(fresh_device) = get_devices_store().lock().unwrap().get(&device_id).cloned() {
                                                                                        action_tx.send(AppAction::InitiateCall {
                                                                                            ip: fresh_device.ip.clone(),
                                                                                            port: fresh_device.port,
                                                                                            number: number.clone(),
                                                                                        });
                                                                                    }
                                                                                }
                                                                            },
                                                                            Icon { icon: IconType::Call, size: 16, color: "currentColor".to_string() }
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }

                                        // Contacts Tab Content
                                        if *phone_sub_tab.read() == "contacts" {
                                            div {
                                                class: "phone-content",
                                                div {
                                                    class: "phone-header",
                                                    h4 { "Contacts ({phone_contacts.read().len()})" }
                                                    if !*auto_sync_contacts.read() {
                                                        button {
                                                            class: "sync-button",
                                                            onclick: {
                                                                let device_id = device.id.clone();
                                                                move |_| {
                                                                    if let Some(fresh_device) = get_devices_store().lock().unwrap().get(&device_id).cloned() {
                                                                        action_tx.send(AppAction::RequestContactsSync {
                                                                            ip: fresh_device.ip.clone(),
                                                                            port: fresh_device.port,
                                                                        });
                                                                    } else {
                                                                        add_notification("Phone", "Device not found. Try refreshing.", "");
                                                                    }
                                                                }
                                                            },
                                                            Icon { icon: IconType::Refresh, size: 14, color: "currentColor".to_string() }
                                                            span { " Sync" }
                                                        }
                                                    }
                                                }

                                                if phone_contacts.read().is_empty() {
                                                    div {
                                                        class: "empty-state",
                                                        span { class: "empty-icon", Icon { icon: IconType::Contact, size: 48, color: "var(--text-tertiary)".to_string() } }
                                                        p { "No contacts synced" }
                                                        if *auto_sync_contacts.read() {
                                                            p { class: "empty-hint", "Contacts will sync automatically" }
                                                        } else {
                                                            p { class: "empty-hint", "Click Sync to load your contacts" }
                                                        }
                                                    }
                                                } else {
                                                    div {
                                                        class: "contacts-list",
                                                        for contact in phone_contacts.read().iter() {
                                                            {
                                                                let name = contact.name.clone();
                                                                let phone = contact.phone_numbers.first()
                                                                    .map(|p| p.number.clone())
                                                                    .unwrap_or_default();
                                                                let starred = contact.starred;

                                                                rsx! {
                                                                    div {
                                                                        class: "contact-item",
                                                                        div {
                                                                            class: "contact-avatar",
                                                                            "{name.chars().next().unwrap_or('?')}"
                                                                        }
                                                                        div {
                                                                            class: "contact-info",
                                                                            div {
                                                                                class: "contact-name",
                                                                                if starred {
                                                                                     Icon { icon: IconType::Star, size: 12, color: "var(--accent)".to_string() }
                                                                                }
                                                                                "{name}"
                                                                            }
                                                                            if !phone.is_empty() {
                                                                                div {
                                                                                    class: "contact-phone",
                                                                                    "{phone}"
                                                                                }
                                                                            }
                                                                        }
                                                                        div {
                                                                            class: "contact-actions",
                                                                            if !phone.is_empty() {
                                                                                button {
                                                                                    class: "contact-action",
                                                                                    title: "Call",
                                                                                    onclick: {
                                                                                        let number = phone.clone();
                                                                                        let device_id = device.id.clone();
                                                                                        move |_| {
                                                                                            if let Some(fresh_device) = get_devices_store().lock().unwrap().get(&device_id).cloned() {
                                                                                                action_tx.send(AppAction::InitiateCall {
                                                                                                    ip: fresh_device.ip.clone(),
                                                                                                    port: fresh_device.port,
                                                                                                    number: number.clone(),
                                                                                                });
                                                                                            }
                                                                                        }
                                                                                    },
                                                                                    Icon { icon: IconType::Call, size: 16, color: "currentColor".to_string() }
                                                                                }
                                                                                button {
                                                                                    class: "contact-action",
                                                                                    title: "Message",
                                                                                    Icon { icon: IconType::Message, size: 16, color: "currentColor".to_string() }
                                                                                }
                                                                            }
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Tab: Settings
                if *active_tab.read() == "settings" {
                    div {
                        class: "settings-section",
                        div {
                            class: "info-card",
                            h3 {
                                Icon { icon: IconType::Refresh, size: 20, color: "currentColor".to_string() }
                                " Updates"
                            }
                            div {
                                class: "info-grid",
                                div { class: "info-label", "Current Version" }
                                div { class: "info-value", "{env!(\"CARGO_PKG_VERSION\")}" }

                                if let Some(info) = update_info.read().as_ref() {
                                    div { class: "info-label", "Latest Version" }
                                    div { class: "info-value", "{info.latest_version}" }

                                    if info.has_update {
                                         div { class: "info-label", "Status" }
                                         div {
                                             class: "info-value",
                                             style: "color: var(--accent); font-weight: bold;",
                                             "Update Available"
                                         }
                                    }
                                }
                            }

                            div {
                                class: "settings-actions",
                                style: "margin-top: 15px; display: flex; gap: 10px;",

                                button {
                                    class: "secondary-button",
                                    onclick: move |_| action_tx.send(AppAction::CheckForUpdates),
                                    "Check for Updates"
                                }

                                if let Some(info) = update_info.read().as_ref() {
                                    if info.has_update {
                                         button {
                                            class: "primary-button",
                                            onclick: move |_| action_tx.send(AppAction::PerformUpdate),
                                            "Update Now"
                                        }
                                    }
                                }
                            }
                        }

                        div {
                            class: "info-card",
                            h3 {
                                Icon { icon: IconType::Desktop, size: 20, color: "currentColor".to_string() }
                                " This Device"
                            }
                            div {
                                class: "info-grid",
                                div { class: "info-label", "Name" }
                                div {
                                    class: "info-value",
                                    style: "display: flex; justify-content: space-between; align-items: center;",
                                    span { "{local_device_name}" }
                                    button {
                                        class: "secondary-button",
                                        style: "padding: 4px 8px; font-size: 0.8rem; margin-left: 8px;",
                                        onclick: move |_| {
                                            rename_text.set(local_device_name.read().clone());
                                            show_rename_dialog.set(true);
                                        },
                                        "Rename"
                                    }
                                }
                                div { class: "info-label", "IP" }
                                div { class: "info-value", "{local_device_ip}" }
                            }
                        }

                        div {
                            class: "info-card",
                            h3 {
                                Icon { icon: IconType::Pair, size: 20, color: "currentColor".to_string() }
                                " Security"
                            }
                            div {
                                class: "info-grid",
                                div { class: "info-label", "Pairing Mode" }
                                div {
                                    class: "info-value",
                                    button {
                                        class: if *pairing_mode.read() { "toggle-button active" } else { "toggle-button" },
                                        onclick: toggle_pairing_mode,
                                        if *pairing_mode.read() { "Enabled" } else { "Disabled" }
                                    }
                                }
                            }
                            p { class: "settings-hint", "Enable to allow new devices to pair with you." }
                        }

                        div {
                            class: "info-card",
                            h3 {
                                Icon { icon: IconType::NavClipboard, size: 20, color: "currentColor".to_string() }
                                " Clipboard Sync"
                            }
                            div {
                                class: "info-grid",
                                div { class: "info-label", "Auto-sync" }
                                div {
                                    class: "info-value",
                                    label {
                                        class: "toggle-switch",
                                        input {
                                            r#type: "checkbox",
                                            checked: "{clipboard_sync_enabled}",
                                            oninput: toggle_clipboard_sync,
                                        }
                                        span { class: "slider" }
                                    }
                                }
                            }
                            p { class: "settings-hint", "Automatically sync clipboard with trusted devices." }
                        }

                        div {
                            class: "info-card",
                            h3 {
                                Icon { icon: IconType::Music, size: 20, color: "currentColor".to_string() }
                                " Media Sharing"
                            }
                            div {
                                class: "info-grid",
                                div { class: "info-label", "Share Status" }
                                div {
                                    class: "info-value",
                                    label {
                                        class: "toggle-switch",
                                        input {
                                            r#type: "checkbox",
                                            checked: "{media_enabled}",
                                            oninput: toggle_media,
                                        }
                                        span { class: "slider" }
                                    }
                                }
                            }
                            p { class: "settings-hint", "Allow other devices to see and control your media." }
                        }

                        if cfg!(target_os = "linux") || cfg!(target_os = "windows") {
                            div {
                                class: "info-card",
                                h3 {
                                    Icon { icon: IconType::Bell, size: 20, color: "currentColor".to_string() }
                                    " Notifications"
                                }
                                div {
                                    class: "info-grid",
                                    div { class: "info-label", "Enable" }
                                    div {
                                        class: "info-value",
                                        label {
                                            class: "toggle-switch",
                                            input {
                                                r#type: "checkbox",
                                                checked: "{notifications_enabled}",
                                                oninput: move |_| {
                                                    let new_val = !*notifications_enabled.read();
                                                    notifications_enabled.set(new_val);
                                                    set_notifications_enabled_setting(new_val);
                                                },
                                            }
                                            span { class: "slider" }
                                        }
                                    }
                                }
                                p { class: "settings-hint", "Show system notifications on this device." }
                            }
                        }

                        div {
                            class: "info-card",
                            h3 {
                                Icon { icon: IconType::NavPhone, size: 20, color: "currentColor".to_string() }
                                " Phone Auto-Sync"
                            }
                            div {
                                class: "info-grid",
                                div { class: "info-label", "Messages" }
                                div {
                                    class: "info-value",
                                    label {
                                        class: "toggle-switch",
                                        input {
                                            r#type: "checkbox",
                                            checked: "{auto_sync_messages}",
                                            oninput: move |_| {
                                                let new_val = !*auto_sync_messages.read();
                                                auto_sync_messages.set(new_val);
                                                set_auto_sync_messages(new_val);
                                            },
                                        }
                                        span { class: "slider" }
                                    }
                                }
                                div { class: "info-label", "Calls" }
                                div {
                                    class: "info-value",
                                    label {
                                        class: "toggle-switch",
                                        input {
                                            r#type: "checkbox",
                                            checked: "{auto_sync_calls}",
                                            oninput: move |_| {
                                                let new_val = !*auto_sync_calls.read();
                                                auto_sync_calls.set(new_val);
                                                set_auto_sync_calls(new_val);
                                            },
                                        }
                                        span { class: "slider" }
                                    }
                                }
                                div { class: "info-label", "Contacts" }
                                div {
                                    class: "info-value",
                                    label {
                                        class: "toggle-switch",
                                        input {
                                            r#type: "checkbox",
                                            checked: "{auto_sync_contacts}",
                                            oninput: move |_| {
                                                let new_val = !*auto_sync_contacts.read();
                                                auto_sync_contacts.set(new_val);
                                                set_auto_sync_contacts(new_val);
                                            },
                                        }
                                        span { class: "slider" }
                                    }
                                }
                            }
                            p { class: "settings-hint", "Automatically sync phone data when opening a device. Hides manual sync buttons when enabled." }
                        }
                    }
                }
            }

            // Notifications (in-app)
            if !cfg!(target_os = "linux") && !cfg!(target_os = "windows") {
                div {
                    class: "notifications-panel",
                    for notification in notifications.read().iter().rev().take(3) {
                        div {
                            class: "notification",
                            key: "{notification.id}",
                            span { class: "notification-icon", "{notification.icon}" }
                            div {
                                class: "notification-content",
                                span { class: "notification-title", "{notification.title}" }
                                span { class: "notification-message", "{notification.message}" }
                            }
                        }
                    }
                }
            }

            // Pairing Requests Modal
            if !pairing_requests.read().is_empty() {
                div {
                    class: "modal-overlay",
                    div {
                        class: "modal-content",
                        h3 { "Pairing Request" }
                        for req in pairing_requests.read().iter() {
                            div {
                                key: "{req.fingerprint}",
                                class: "pairing-request",
                                p { "Device: {req.device_name}" }
                                p { class: "fingerprint", "ID: {req.fingerprint}" }
                                div {
                                    class: "modal-actions",
                                    button {
                                        class: "secondary-button",
                                        onclick: {
                                            let req = req.clone();
                                            move |_| {
                                                action_tx.send(AppAction::RejectDevice {
                                                    fingerprint: req.fingerprint.clone(),
                                                    device_id: req.device_id.clone(),
                                                });
                                                get_pairing_requests().lock().unwrap().retain(|r| r.fingerprint != req.fingerprint);
                                            }
                                        },
                                        "Reject"
                                    }
                                    button {
                                        class: "primary-button",
                                        onclick: {
                                            let req = req.clone();
                                            move |_| {
                                                action_tx.send(AppAction::TrustDevice {
                                                    fingerprint: req.fingerprint.clone(),
                                                    name: req.device_name.clone(),
                                                    device_id: req.device_id.clone()
                                                });
                                                get_pairing_requests().lock().unwrap().retain(|r| r.fingerprint != req.fingerprint);
                                            }
                                        },
                                        "Trust"
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // File Transfer Requests Modal
            if !file_transfer_requests.read().is_empty() {
                div {
                    class: "modal-overlay",
                    div {
                        class: "modal-content",
                        h3 { "Incoming File Transfer" }
                        for req in file_transfer_requests.read().iter() {
                            div {
                                key: "{req.id}",
                                class: "pairing-request", // Reuse styling
                                p { "From: {req.from_device}" }
                                p { class: "fingerprint", "File: {req.filename}" }
                                p { class: "fingerprint", "Size: {req.size} bytes" }
                                div {
                                    class: "modal-actions",
                                    button {
                                        class: "secondary-button",
                                        onclick: {
                                            let req = req.clone();
                                            move |_| {
                                                action_tx.send(AppAction::RejectFileTransfer { transfer_id: req.id.clone() });
                                            }
                                        },
                                        "Reject"
                                    }
                                    button {
                                        class: "primary-button",
                                        onclick: {
                                            let req = req.clone();
                                            move |_| {
                                                action_tx.send(AppAction::AcceptFileTransfer { transfer_id: req.id.clone() });
                                            }
                                        },
                                        "Accept"
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Incoming/Active Call Modal
            if let Some(call) = active_call.read().clone() {
                div {
                    class: "modal-overlay call-overlay",
                    div {
                        class: "modal-content call-modal",
                        // Call state header
                        div {
                            class: "call-modal-header",
                            match call.state {
                                ActiveCallState::Ringing => rsx! {
                                    div { class: "call-state-icon ringing", Icon { icon: IconType::Call, size: 48, color: "currentColor".to_string() } }
                                    h3 { "Incoming Call" }
                                },
                                ActiveCallState::Dialing => rsx! {
                                    div { class: "call-state-icon dialing", Icon { icon: IconType::NavPhone, size: 48, color: "currentColor".to_string() } }
                                    h3 { "Dialing..." }
                                },
                                ActiveCallState::Connected => rsx! {
                                    div { class: "call-state-icon connected", Icon { icon: IconType::VolumeUp, size: 48, color: "currentColor".to_string() } }
                                    h3 { "Call in Progress" }
                                },
                                ActiveCallState::OnHold => rsx! {
                                    div { class: "call-state-icon on-hold", Icon { icon: IconType::Pause, size: 48, color: "currentColor".to_string() } }
                                    h3 { "On Hold" }
                                },
                                ActiveCallState::Ended => rsx! {
                                    div { class: "call-state-icon ended", Icon { icon: IconType::Unpair, size: 48, color: "currentColor".to_string() } }
                                    h3 { "Call Ended" }
                                },
                            }
                        }

                        // Caller info
                        div {
                            class: "call-caller-info",
                            div {
                                class: "call-avatar",
                                {call.contact_name.as_ref().and_then(|n| n.chars().next()).unwrap_or('?').to_string()}
                            }
                            div {
                                class: "call-caller-details",
                                p {
                                    class: "call-caller-name",
                                    {call.contact_name.clone().unwrap_or_else(|| call.number.clone())}
                                }
                                p {
                                    class: "call-caller-number",
                                    {call.number.clone()}
                                }
                            }
                        }

                        // Call duration for connected calls
                        if call.state == ActiveCallState::Connected && call.duration > 0 {
                            div {
                                class: "call-duration",
                                {format!("{}:{:02}", call.duration / 60, call.duration % 60)}
                            }
                        }

                        // Action buttons
                        div {
                            class: "call-modal-actions",
                            match call.state {
                                ActiveCallState::Ringing => {
                                    // Show Answer and Reject buttons for incoming ringing calls
                                    rsx! {
                                        button {
                                            class: "call-button reject",
                                            onclick: {
                                                let selected = selected_device.read().clone();
                                                move |_| {
                                                    if let Some(ref device) = selected {
                                                        action_tx.send(AppAction::SendCallAction {
                                                            ip: device.ip.clone(),
                                                            port: device.port,
                                                            action: CallAction::Reject,
                                                        });
                                                    }
                                                }
                                            },
                                            span { class: "call-btn-icon", Icon { icon: IconType::Unpair, size: 20, color: "currentColor".to_string() } }
                                            span { "Reject" }
                                        }
                                        button {
                                            class: "call-button answer",
                                            onclick: {
                                                let selected = selected_device.read().clone();
                                                move |_| {
                                                    if let Some(ref device) = selected {
                                                        action_tx.send(AppAction::SendCallAction {
                                                            ip: device.ip.clone(),
                                                            port: device.port,
                                                            action: CallAction::Answer,
                                                        });
                                                    }
                                                }
                                            },
                                            span { class: "call-btn-icon", Icon { icon: IconType::Call, size: 20, color: "currentColor".to_string() } }
                                            span { "Answer" }
                                        }
                                    }
                                },
                                ActiveCallState::Connected | ActiveCallState::OnHold | ActiveCallState::Dialing => {
                                    // Show End Call button for active calls
                                    rsx! {
                                        button {
                                            class: "call-button end-call",
                                            onclick: {
                                                let selected = selected_device.read().clone();
                                                move |_| {
                                                    if let Some(ref device) = selected {
                                                        action_tx.send(AppAction::SendCallAction {
                                                            ip: device.ip.clone(),
                                                            port: device.port,
                                                            action: CallAction::HangUp,
                                                        });
                                                    }
                                                }
                                            },
                                            span { class: "call-btn-icon", Icon { icon: IconType::Unpair, size: 20, color: "currentColor".to_string() } }
                                            span { "End Call" }
                                        }
                                    }
                                },
                                ActiveCallState::Ended => {
                                    // Show dismiss button for ended calls
                                    rsx! {
                                        button {
                                            class: "call-button dismiss",
                                            onclick: move |_| {
                                                set_active_call(None);
                                            },
                                            span { "Dismiss" }
                                        }
                                    }
                                },
                            }
                        }
                    }
                }
            }

            if *show_rename_dialog.read() {
                div {
                    class: "modal-overlay",
                    div {
                        class: "modal-content",
                        h3 { "Rename Device" }
                        div {
                            style: "margin: 16px 0;",
                            input {
                                r#type: "text",
                                style: "width: 100%; padding: 8px; border: 1px solid var(--border-color); border-radius: 4px; background: var(--bg-secondary); color: var(--text-primary);",
                                value: "{rename_text}",
                                oninput: move |e| rename_text.set(e.value().clone()),
                                placeholder: "Enter new name"
                            }
                        }
                        div {
                            class: "modal-actions",
                            button {
                                class: "secondary-button",
                                onclick: move |_| show_rename_dialog.set(false),
                                "Cancel"
                            }
                            button {
                                class: "primary-button",
                                onclick: move |_| {
                                    let new_name = rename_text.read().trim().to_string();
                                    if !new_name.is_empty() {
                                        local_device_name.set(new_name.clone());
                                        action_tx.send(AppAction::RenameDevice { new_name });
                                        show_rename_dialog.set(false);
                                    }
                                },
                                "Save"
                            }
                        }
                    }
                }
            }

            if *show_send_dialog.read() {
                FileDialog {
                    device: send_target_device.read().clone(),
                    is_folder: *send_dialog_is_folder.read(),
                    on_close: move |_| {
                        show_send_dialog.set(false);
                        send_target_device.set(None);
                    },
                    on_send: move |path: String| {
                        let final_path = path.clone();
                        let success = true;

                        if let Some(ref device) = *send_target_device.read()
                            && success
                        {
                            action_tx.send(AppAction::SendFile {
                                ip: device.ip.clone(),
                                port: device.port,
                                path: final_path
                            });
                        }
                        show_send_dialog.set(false);
                        send_target_device.set(None);
                    }
                }
            }
        }
    }
}
