#![allow(non_snake_case)]

mod callbacks;
mod components;
mod state;
mod utils;

use callbacks::{AppClipboardCallback, AppDiscoveryCallback, AppFileTransferCallback};
use components::{DeviceCard, FileDialog};
use connected_core::facade::{ClipboardCallback, FileTransferCallback};
use dioxus::prelude::*;
use state::*;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use tracing::{error, info, warn};
use utils::{get_device_icon, get_system_clipboard};

fn main() {
    // Initialize logging - filter out noisy mdns-sd errors
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("info".parse().unwrap())
                .add_directive("connected_core=debug".parse().unwrap())
                .add_directive("connected_desktop=debug".parse().unwrap())
                .add_directive("mdns_sd=warn".parse().unwrap()),
        )
        .init();

    info!("Connected Desktop starting...");

    // Configure Dioxus desktop
    let config = dioxus::desktop::Config::new()
        .with_window(
            dioxus::desktop::WindowBuilder::new()
                .with_title("Connected")
                .with_inner_size(dioxus::desktop::LogicalSize::new(480.0, 720.0))
                .with_min_inner_size(dioxus::desktop::LogicalSize::new(400.0, 600.0))
                .with_resizable(true),
        )
        .with_disable_context_menu(true);

    LaunchBuilder::desktop().with_cfg(config).launch(App);
}

#[component]
fn App() -> Element {
    // Application state
    let mut initialized = use_signal(|| false);
    let mut local_device_name = use_signal(|| "Desktop".to_string());
    let mut local_device_ip = use_signal(|| String::new());
    let mut devices_list = use_signal(Vec::<DeviceInfo>::new);
    let mut selected_device = use_signal(|| None::<DeviceInfo>);
    let mut active_tab = use_signal(|| "devices".to_string());
    let mut transfer_status = use_signal(|| TransferStatus::Idle);
    let mut clipboard_sync_device = use_signal(|| None::<DeviceInfo>);
    let mut notifications = use_signal(Vec::<Notification>::new);
    let mut show_send_dialog = use_signal(|| false);
    let mut clipboard_text = use_signal(String::new);
    let mut show_clipboard_dialog = use_signal(|| false);
    let mut discovery_active = use_signal(|| false);

    // Initialize core on mount
    use_effect(move || {
        if *initialized.read() {
            return;
        }

        let name = std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("HOST"))
            .or_else(|_| std::env::var("COMPUTERNAME"))
            .unwrap_or_else(|_| "Desktop".into());

        let name_clone = name.clone();

        thread::spawn(move || {
            match connected_core::facade::initialize(name_clone.clone(), "linux".into(), 44444) {
                Ok(_) => {
                    info!("Core initialized as {}", name_clone);

                    // Start file receiver
                    let save_dir = dirs::download_dir()
                        .unwrap_or_else(|| PathBuf::from("."))
                        .to_string_lossy()
                        .to_string();

                    let file_cb = Box::new(AppFileTransferCallback);
                    let clip_cb = Box::new(AppClipboardCallback);

                    connected_core::facade::register_clipboard_receiver(clip_cb);
                    if let Err(e) = connected_core::facade::start_file_receiver(save_dir, file_cb) {
                        error!("Failed to start file receiver: {}", e);
                    }

                    // Get local device info
                    if let Ok(device) = connected_core::facade::get_local_device() {
                        info!("Local device: {} at {}", device.name, device.ip);
                    }

                    // Clear any stale devices from previous sessions
                    get_devices_store().lock().unwrap().clear();
                    info!("Cleared previous device list");

                    // Auto-start discovery - always on when app is running
                    let callback = Box::new(AppDiscoveryCallback);
                    match connected_core::facade::start_discovery(callback) {
                        Ok(_) => {
                            info!("Discovery started automatically");
                            // Devices will be populated via callbacks as they're discovered
                        }
                        Err(e) => error!("Failed to start discovery: {}", e),
                    }
                }
                Err(e) => {
                    error!("Core initialization failed: {}", e);
                    add_notification("Initialization Failed", &e.to_string(), "❌");
                }
            }
        });

        // Get local device info for display
        if let Ok(device) = std::thread::spawn(|| connected_core::facade::get_local_device()).join()
        {
            if let Ok(d) = device {
                local_device_ip.set(d.ip.clone());
            }
        }

        local_device_name.set(name);
        initialized.set(true);
        discovery_active.set(true);
    });

    // Poll for state changes
    use_future(move || async move {
        loop {
            // Update devices list from store (populated by callbacks)
            let list: Vec<DeviceInfo> = get_devices_store()
                .lock()
                .unwrap()
                .values()
                .cloned()
                .collect();
            devices_list.set(list);

            // Update transfer status
            let status = get_transfer_status().lock().unwrap().clone();
            transfer_status.set(status);

            // Update notifications - auto-dismiss after 5 seconds
            {
                let mut notifs = get_notifications().lock().unwrap();
                let now = std::time::Instant::now();
                notifs.retain(|n| now.duration_since(n.timestamp).as_secs() < 5);
            }
            let notifs = get_notifications().lock().unwrap().clone();
            notifications.set(notifs);

            // Clipboard sync loop
            if let Some(ref device) = *clipboard_sync_device.read() {
                let current_clip = get_system_clipboard();
                let last_clip = get_last_clipboard().lock().unwrap().clone();

                if !current_clip.is_empty() && current_clip != last_clip {
                    *get_last_clipboard().lock().unwrap() = current_clip.clone();

                    let ip = device.ip.clone();
                    let port = device.port + 1;
                    let text = current_clip.clone();

                    thread::spawn(move || {
                        struct SyncClipboardCallback;
                        impl ClipboardCallback for SyncClipboardCallback {
                            fn on_clipboard_received(&self, _: String, _: String) {}
                            fn on_clipboard_sent(&self, success: bool, error: Option<String>) {
                                if success {
                                    info!("Clipboard synced");
                                } else {
                                    warn!("Clipboard sync failed: {:?}", error);
                                }
                            }
                        }
                        let _ = connected_core::facade::send_clipboard(
                            ip,
                            port,
                            text,
                            Box::new(SyncClipboardCallback),
                        );
                    });
                }
            }

            async_std::task::sleep(Duration::from_millis(500)).await;
        }
    });

    let toggle_clipboard_sync = move |_| {
        if clipboard_sync_device.read().is_some() {
            clipboard_sync_device.set(None);
            add_notification("Clipboard Sync", "Sync stopped", "📋");
        } else if let Some(device) = selected_device.read().clone() {
            *get_last_clipboard().lock().unwrap() = get_system_clipboard();
            clipboard_sync_device.set(Some(device.clone()));
            add_notification(
                "Clipboard Sync",
                &format!("Syncing with {}", device.name),
                "📋",
            );
        }
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
                    div { class: "logo", "⚡" }
                    h1 { "Connected" }
                }

                // Local device info
                div {
                    class: "local-device",
                    div {
                        class: "local-device-icon",
                        {get_device_icon("linux")}
                    }
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
                        class: if *active_tab.read() == "devices" { "nav-item active" } else { "nav-item" },
                        onclick: move |_| active_tab.set("devices".to_string()),
                        span { class: "nav-icon", "📱" }
                        span { "Devices" }
                        if !devices_list.read().is_empty() {
                            span { class: "nav-badge", "{devices_list.read().len()}" }
                        }
                    }
                    button {
                        class: if *active_tab.read() == "transfers" { "nav-item active" } else { "nav-item" },
                        onclick: move |_| active_tab.set("transfers".to_string()),
                        span { class: "nav-icon", "📁" }
                        span { "Transfers" }
                    }
                    button {
                        class: if *active_tab.read() == "clipboard" { "nav-item active" } else { "nav-item" },
                        onclick: move |_| active_tab.set("clipboard".to_string()),
                        span { class: "nav-icon", "📋" }
                        span { "Clipboard" }
                        if clipboard_sync_device.read().is_some() {
                            span { class: "nav-badge sync", "●" }
                        }
                    }
                    button {
                        class: if *active_tab.read() == "settings" { "nav-item active" } else { "nav-item" },
                        onclick: move |_| active_tab.set("settings".to_string()),
                        span { class: "nav-icon", "⚙️" }
                        span { "Settings" }
                    }
                }

                // Status indicator (replaces scan button)
                div {
                    class: "sidebar-footer",
                    div {
                        class: "discovery-status",
                        if *initialized.read() && *discovery_active.read() {
                            span { class: "status-indicator active", "●" }
                            span { "Discovering nearby devices" }
                        } else if *initialized.read() {
                            span { class: "status-indicator", "○" }
                            span { "Ready" }
                        } else {
                            span { class: "status-indicator", "◌" }
                            span { "Starting..." }
                        }
                    }
                }
            }

            // Main content
            main {
                class: "main-content",

                // Tab: Devices
                if *active_tab.read() == "devices" {
                    div {
                        class: "content-header",
                        h2 { "Nearby Devices" }
                        span { class: "content-subtitle", "{devices_list.read().len()} device(s) found" }
                    }

                    if devices_list.read().is_empty() {
                        div {
                            class: "empty-state",
                            div { class: "empty-icon searching", "📡" }
                            h3 { "Looking for Devices" }
                            p { "Make sure other devices are on the same network and have Connected running." }
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
                                    is_selected: selected_device.read().as_ref().map(|d| d.id == device.id).unwrap_or(false),
                                    on_select: move |d: DeviceInfo| {
                                        selected_device.set(Some(d));
                                    },
                                    on_send_file: move |d: DeviceInfo| {
                                        selected_device.set(Some(d));
                                        show_send_dialog.set(true);
                                    },
                                    on_send_clipboard: move |d: DeviceInfo| {
                                        selected_device.set(Some(d));
                                        clipboard_text.set(get_system_clipboard());
                                        show_clipboard_dialog.set(true);
                                    },
                                }
                            }
                        }
                    }
                }

                // Tab: Transfers
                if *active_tab.read() == "transfers" {
                    div {
                        class: "content-header",
                        h2 { "File Transfers" }
                        span { class: "content-subtitle", "Send and receive files" }
                    }

                    div {
                        class: "transfers-section",

                        // Current transfer status
                        match &*transfer_status.read() {
                            TransferStatus::Idle => rsx! {
                                div {
                                    class: "transfer-idle",
                                    div { class: "transfer-icon", "📂" }
                                    p { "No active transfers" }
                                }
                            },
                            TransferStatus::Starting(filename) => rsx! {
                                div {
                                    class: "transfer-active",
                                    div { class: "transfer-icon spinning", "⏳" }
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
                                    div { class: "transfer-icon", "✅" }
                                    p { "{filename} received successfully!" }
                                }
                            },
                            TransferStatus::Failed(error) => rsx! {
                                div {
                                    class: "transfer-failed",
                                    div { class: "transfer-icon", "❌" }
                                    p { "Transfer failed: {error}" }
                                }
                            },
                        }

                        // Send file section
                        div {
                            class: "send-file-section",
                            h3 { "Send a File" }
                            if let Some(ref device) = *selected_device.read() {
                                p { "Send to: {device.name}" }
                                button {
                                    class: "primary-button",
                                    onclick: move |_| show_send_dialog.set(true),
                                    "Choose File"
                                }
                            } else {
                                p { class: "muted", "Select a device first" }
                            }
                        }

                        // Download location
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

                // Tab: Clipboard
                if *active_tab.read() == "clipboard" {
                    div {
                        class: "content-header",
                        h2 { "Clipboard Sync" }
                        span { class: "content-subtitle", "Share your clipboard across devices" }
                    }

                    div {
                        class: "clipboard-section",

                        // Sync status
                        div {
                            class: if clipboard_sync_device.read().is_some() { "sync-status active" } else { "sync-status" },
                            if let Some(ref device) = *clipboard_sync_device.read() {
                                div { class: "sync-icon", "🔄" }
                                div {
                                    class: "sync-info",
                                    span { class: "sync-label", "Syncing with" }
                                    span { class: "sync-device", "{device.name}" }
                                }
                                button {
                                    class: "stop-sync-button",
                                    onclick: toggle_clipboard_sync,
                                    "Stop Sync"
                                }
                            } else {
                                div { class: "sync-icon muted", "📋" }
                                div {
                                    class: "sync-info",
                                    span { class: "sync-label muted", "Not syncing" }
                                    if let Some(ref device) = *selected_device.read() {
                                        button {
                                            class: "primary-button",
                                            onclick: toggle_clipboard_sync,
                                            "Sync with {device.name}"
                                        }
                                    } else {
                                        span { class: "sync-hint", "Select a device to start syncing" }
                                    }
                                }
                            }
                        }

                        // Manual send
                        div {
                            class: "manual-clipboard",
                            h3 { "Send Clipboard" }
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
                                    "📋 Paste from Clipboard"
                                }
                                if let Some(ref device) = *selected_device.read() {
                                    button {
                                        class: "primary-button",
                                        disabled: clipboard_text.read().is_empty(),
                                        onclick: {
                                            let device = device.clone();
                                            move |_| {
                                                let ip = device.ip.clone();
                                                let port = device.port + 1;
                                                let text = clipboard_text.read().clone();
                                                if !text.is_empty() {
                                                    thread::spawn(move || {
                                                        struct SendCallback;
                                                        impl ClipboardCallback for SendCallback {
                                                            fn on_clipboard_received(&self, _: String, _: String) {}
                                                            fn on_clipboard_sent(&self, success: bool, error: Option<String>) {
                                                                if success {
                                                                    info!("Clipboard sent");
                                                                } else {
                                                                    warn!("Clipboard send failed: {:?}", error);
                                                                }
                                                            }
                                                        }
                                                        let _ = connected_core::facade::send_clipboard(ip, port, text, Box::new(SendCallback));
                                                    });
                                                    add_notification("Clipboard", "Sending clipboard...", "📤");
                                                }
                                            }
                                        },
                                        "Send to {device.name}"
                                    }
                                }
                            }
                        }
                    }
                }

                // Tab: Settings/Info
                if *active_tab.read() == "settings" {
                    div {
                        class: "content-header",
                        h2 { "Settings & Info" }
                        span { class: "content-subtitle", "Device information and configuration" }
                    }

                    div {
                        class: "settings-section",

                        // Local device info
                        div {
                            class: "info-card",
                            h3 { "📱 This Device" }
                            div {
                                class: "info-grid",
                                div { class: "info-label", "Name" }
                                div { class: "info-value", "{local_device_name}" }
                                div { class: "info-label", "IP Address" }
                                div { class: "info-value", "{local_device_ip}" }
                                div { class: "info-label", "Port" }
                                div { class: "info-value", "44444" }
                                div { class: "info-label", "File Port" }
                                div { class: "info-value", "44445" }
                                div { class: "info-label", "Type" }
                                div { class: "info-value", "Linux Desktop" }
                                div { class: "info-label", "Status" }
                                div { class: "info-value",
                                    if *initialized.read() {
                                        span { class: "status-online", "● Online" }
                                    } else {
                                        span { class: "status-offline", "○ Offline" }
                                    }
                                }
                            }
                        }

                        // Network info
                        div {
                            class: "info-card",
                            h3 { "🌐 Network" }
                            div {
                                class: "info-grid",
                                div { class: "info-label", "Protocol" }
                                div { class: "info-value", "QUIC (UDP)" }
                                div { class: "info-label", "Discovery" }
                                div { class: "info-value", "mDNS (_connected._udp.local.)" }
                                div { class: "info-label", "Encryption" }
                                div { class: "info-value", "TLS 1.3" }
                                div { class: "info-label", "Discovered Devices" }
                                div { class: "info-value", "{devices_list.read().len()}" }
                            }
                        }

                        // Download location
                        div {
                            class: "info-card",
                            h3 { "📂 Storage" }
                            div {
                                class: "info-grid",
                                div { class: "info-label", "Download Location" }
                                div { class: "info-value path",
                                    {dirs::download_dir().unwrap_or_else(|| PathBuf::from(".")).display().to_string()}
                                }
                            }
                        }

                        // Version info
                        div {
                            class: "info-card",
                            h3 { "ℹ️ About" }
                            div {
                                class: "info-grid",
                                div { class: "info-label", "Version" }
                                div { class: "info-value", {env!("CARGO_PKG_VERSION")} }
                                div { class: "info-label", "Built with" }
                                div { class: "info-value", "Rust + Dioxus" }
                            }
                        }
                    }
                }
            }

            // Notifications panel
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

            // File send dialog
            if *show_send_dialog.read() {
                FileDialog {
                    device: selected_device.read().clone(),
                    on_close: move |_| show_send_dialog.set(false),
                    on_send: move |path: String| {
                        if let Some(ref device) = *selected_device.read() {
                            let ip = device.ip.clone();
                            let port = device.port;
                            thread::spawn(move || {
                                struct SendCallback;
                                impl FileTransferCallback for SendCallback {
                                    fn on_transfer_starting(&self, filename: String, size: u64) {
                                        *get_transfer_status().lock().unwrap() = TransferStatus::Starting(filename.clone());
                                        info!("Sending: {} ({} bytes)", filename, size);
                                    }
                                    fn on_transfer_progress(&self, bytes: u64, total: u64) {
                                        let percent = if total > 0 { (bytes as f32 / total as f32) * 100.0 } else { 0.0 };
                                        let mut status = get_transfer_status().lock().unwrap();
                                        let filename = match &*status {
                                            TransferStatus::Starting(f) => f.clone(),
                                            TransferStatus::InProgress { filename, .. } => filename.clone(),
                                            _ => "file".to_string(),
                                        };
                                        *status = TransferStatus::InProgress { filename, percent };
                                    }
                                    fn on_transfer_completed(&self, filename: String, _: u64) {
                                        *get_transfer_status().lock().unwrap() = TransferStatus::Completed(filename);
                                        add_notification("Sent", "File sent successfully!", "✅");
                                    }
                                    fn on_transfer_failed(&self, err: String) {
                                        *get_transfer_status().lock().unwrap() = TransferStatus::Failed(err.clone());
                                        add_notification("Failed", &err, "❌");
                                    }
                                    fn on_transfer_cancelled(&self) {
                                        *get_transfer_status().lock().unwrap() = TransferStatus::Idle;
                                    }
                                }
                                let _ = connected_core::facade::send_file(ip, port, path, Box::new(SendCallback));
                            });
                        }
                        show_send_dialog.set(false);
                    }
                }
            }
        }
    }
}
