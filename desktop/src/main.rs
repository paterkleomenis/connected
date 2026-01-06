#![allow(non_snake_case)]

mod components;
mod state;
mod utils;

use components::{DeviceCard, FileDialog};
use connected_core::{ConnectedClient, ConnectedEvent, DeviceType};
use dioxus::prelude::*;
use futures_util::StreamExt;
use state::*;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};
use utils::{get_device_icon, get_system_clipboard};

fn main() {
    // Initialize logging
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

#[derive(Clone, Debug)]
enum AppAction {
    Init,
    SendFile { ip: String, port: u16, path: String },
    SendClipboard { ip: String, port: u16, text: String },
    ToggleClipboardSync { device: Option<DeviceInfo> },
}

#[component]
fn App() -> Element {
    // UI State
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

    // The Controller

    let action_tx = use_coroutine(move |mut rx: UnboundedReceiver<AppAction>| async move {
        let mut client: Option<Arc<ConnectedClient>> = None;

        let mut _clipboard_synced_peer: Option<String> = None; // IP of synced peer

        while let Some(action) = rx.next().await {
            match action {
                AppAction::Init => {
                    if client.is_some() {
                        continue;
                    }

                    let name = std::env::var("HOSTNAME")
                        .or_else(|_| std::env::var("HOST"))
                        .or_else(|_| std::env::var("COMPUTERNAME"))
                        .unwrap_or_else(|_| "Desktop".into());

                    local_device_name.set(name.clone());

                    match ConnectedClient::new(name.clone(), DeviceType::Linux, 44444).await {
                        Ok(c) => {
                            info!("Core initialized");

                            client = Some(c.clone());

                            initialized.set(true);

                            discovery_active.set(true);

                            local_device_ip.set(c.local_device().ip.to_string());

                            // Start File Receiver (Legacy helper)
                            let save_dir =
                                dirs::download_dir().unwrap_or_else(|| PathBuf::from("."));
                            let _ = c.start_file_receiver(save_dir).await;

                            // Subscribe to events
                            let mut events = c.subscribe();

                            // Spawn event loop
                            tokio::spawn(async move {
                                while let Ok(event) = events.recv().await {
                                    match event {
                                        ConnectedEvent::DeviceFound(d) => {
                                            let info: DeviceInfo = d.clone().into();
                                            // Update devices list signal
                                            // Note: Direct signal update from background task might need schedule_update in some Dioxus versions,
                                            // but use_coroutine context usually handles it if we are in the main scope?
                                            // Actually we are in a spawned task inside coroutine. We need to be careful.
                                            // For safety in Dioxus Desktop, direct signal set is usually thread-safe.

                                            // Update Store
                                            let mut store = get_devices_store().lock().unwrap();
                                            if !store.contains_key(&info.id) {
                                                add_notification(
                                                    "Device Found",
                                                    &format!("{} available", info.name),
                                                    "📱",
                                                );
                                            }
                                            store.insert(info.id.clone(), info);
                                        }
                                        ConnectedEvent::DeviceLost(id) => {
                                            let mut store = get_devices_store().lock().unwrap();
                                            if let Some(d) = store.remove(&id) {
                                                add_notification(
                                                    "Device Lost",
                                                    &format!("{} disconnected", d.name),
                                                    "📴",
                                                );
                                            }
                                        }
                                        ConnectedEvent::TransferStarting {
                                            filename,
                                            direction,
                                            ..
                                        } => {
                                            use connected_core::events::TransferDirection;
                                            if direction == TransferDirection::Incoming {
                                                *get_transfer_status().lock().unwrap() =
                                                    TransferStatus::Starting(filename.clone());
                                                add_notification(
                                                    "Transfer Starting",
                                                    &format!("Receiving {}", filename),
                                                    "📥",
                                                );
                                            } else {
                                                *get_transfer_status().lock().unwrap() =
                                                    TransferStatus::Starting(filename.clone());
                                            }
                                        }
                                        ConnectedEvent::TransferProgress {
                                            bytes_transferred,
                                            total_size,
                                            id: _,
                                        } => {
                                            let percent = if total_size > 0 {
                                                (bytes_transferred as f32 / total_size as f32)
                                                    * 100.0
                                            } else {
                                                0.0
                                            };
                                            let mut status = get_transfer_status().lock().unwrap();
                                            // Only update if we are in a state that has a filename
                                            let current_filename = match &*status {
                                                TransferStatus::Starting(f) => Some(f.clone()),
                                                TransferStatus::InProgress { filename, .. } => {
                                                    Some(filename.clone())
                                                }
                                                _ => None,
                                            };

                                            if let Some(filename) = current_filename {
                                                *status = TransferStatus::InProgress {
                                                    filename,
                                                    percent,
                                                };
                                            }
                                        }
                                        ConnectedEvent::TransferCompleted { filename, .. } => {
                                            *get_transfer_status().lock().unwrap() =
                                                TransferStatus::Completed(filename.clone());
                                            add_notification(
                                                "Transfer Complete",
                                                &format!("{} finished", filename),
                                                "✅",
                                            );
                                        }
                                        ConnectedEvent::TransferFailed { error, .. } => {
                                            *get_transfer_status().lock().unwrap() =
                                                TransferStatus::Failed(error.clone());
                                            add_notification("Transfer Failed", &error, "❌");
                                        }
                                        ConnectedEvent::ClipboardReceived {
                                            content,
                                            from_device,
                                        } => {
                                            utils::set_system_clipboard(&content);
                                            *get_last_clipboard().lock().unwrap() = content.clone();
                                            add_notification(
                                                "Clipboard",
                                                &format!("Received from {}", from_device),
                                                "📋",
                                            );
                                        }
                                        ConnectedEvent::Error(msg) => {
                                            error!("System error: {}", msg);
                                        }
                                    }
                                }
                            });
                        }
                        Err(e) => {
                            error!("Failed to init: {}", e);
                            add_notification("Init Failed", &e.to_string(), "❌");
                        }
                    }
                }
                AppAction::SendFile { ip, port, path } => {
                    if let Some(c) = &client {
                        let c = c.clone();
                        tokio::spawn(async move {
                            if let Ok(ip_addr) = ip.parse() {
                                let _ = c.send_file(ip_addr, port, PathBuf::from(path)).await;
                            }
                        });
                    }
                }
                AppAction::SendClipboard { ip, port, text } => {
                    if let Some(c) = &client {
                        let c = c.clone();
                        tokio::spawn(async move {
                            if let Ok(ip_addr) = ip.parse() {
                                let _ = c.send_clipboard(ip_addr, port, text).await;
                            }
                        });
                    }
                }
                AppAction::ToggleClipboardSync { device } => {
                    if let Some(d) = device {
                        _clipboard_synced_peer = Some(d.ip.clone());
                        // TODO: Implement actual sync loop or registration here?
                        // For now, the UI Poller will handle the *sending* part by invoking SendClipboard
                        // This action just updates internal state if needed.
                    } else {
                        _clipboard_synced_peer = None;
                    }
                }
            }
        }
    });

    // Start Init
    use_effect(move || {
        action_tx.send(AppAction::Init);
    });

    // UI Poller (Reduced scope: just syncs UI state from global stores and checks clipboard)
    // We still need this because Dioxus signals don't automatically update when the `get_devices_store()` Mutex changes
    // unless we wrap the store in a Signal or trigger an update.
    use_future(move || async move {
        loop {
            // Update devices list
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

            // Update notifications
            {
                let mut notifs = get_notifications().lock().unwrap();
                let now = std::time::Instant::now();
                notifs.retain(|n| now.duration_since(n.timestamp).as_secs() < 5);
            }
            notifications.set(get_notifications().lock().unwrap().clone());

            // Clipboard Sync Check
            if let Some(ref device) = *clipboard_sync_device.read() {
                let current_clip = get_system_clipboard();
                let last_clip = get_last_clipboard().lock().unwrap().clone();
                if !current_clip.is_empty() && current_clip != last_clip {
                    *get_last_clipboard().lock().unwrap() = current_clip.clone();
                    action_tx.send(AppAction::SendClipboard {
                        ip: device.ip.clone(),
                        port: device.port + 1,
                        text: current_clip,
                    });
                }
            }

            async_std::task::sleep(Duration::from_millis(200)).await;
        }
    });

    let toggle_clipboard_sync = move |_| {
        if clipboard_sync_device.read().is_some() {
            clipboard_sync_device.set(None);
            action_tx.send(AppAction::ToggleClipboardSync { device: None });
            add_notification("Clipboard Sync", "Sync stopped", "📋");
        } else if let Some(device) = selected_device.read().clone() {
            *get_last_clipboard().lock().unwrap() = get_system_clipboard();
            clipboard_sync_device.set(Some(device.clone()));
            action_tx.send(AppAction::ToggleClipboardSync {
                device: Some(device.clone()),
            });
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

                // Status indicator
                div {
                    class: "sidebar-footer",
                    div {
                        class: "discovery-status",
                        if *initialized.read() && *discovery_active.read() {
                            span { class: "status-indicator active", "●" }
                            span { "Discovering" }
                        } else {
                            span { class: "status-indicator", "○" }
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
                        span { class: "content-subtitle", "Share your clipboard" }
                    }

                    div {
                        class: "clipboard-section",
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
                                                    action_tx.send(AppAction::SendClipboard { ip, port, text });
                                                    add_notification("Clipboard", "Sending...", "📤");
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

                // Tab: Settings
                if *active_tab.read() == "settings" {
                    div {
                        class: "settings-section",
                        div {
                            class: "info-card",
                            h3 { "📱 This Device" }
                            div {
                                class: "info-grid",
                                div { class: "info-label", "Name" }
                                div { class: "info-value", "{local_device_name}" }
                                div { class: "info-label", "IP" }
                                div { class: "info-value", "{local_device_ip}" }
                            }
                        }
                    }
                }
            }

            // Notifications
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

            if *show_send_dialog.read() {
                FileDialog {
                    device: selected_device.read().clone(),
                    on_close: move |_| show_send_dialog.set(false),
                    on_send: move |path: String| {
                        if let Some(ref device) = *selected_device.read() {
                            action_tx.send(AppAction::SendFile {
                                ip: device.ip.clone(),
                                port: device.port,
                                path
                            });
                        }
                        show_send_dialog.set(false);
                    }
                }
            }
        }
    }
}
