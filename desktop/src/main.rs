#![allow(non_snake_case)]

mod components;
mod controller;
mod fs_provider;
mod state;
mod utils;

use components::{DeviceCard, FileBrowser, FileDialog};
use controller::{app_controller, AppAction};
use dioxus::prelude::*;
use state::*;
use std::path::PathBuf;
use std::time::Duration;
use tracing::{debug, info};
use utils::get_system_clipboard;

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

#[component]
fn App() -> Element {
    // UI State
    let initialized = use_signal(|| false);
    let local_device_name = use_signal(|| "Desktop".to_string());
    let local_device_ip = use_signal(|| String::new());
    let mut devices_list = use_signal(Vec::<DeviceInfo>::new);
    let mut selected_device = use_signal(|| None::<DeviceInfo>);
    let mut active_tab = use_signal(|| "devices".to_string());
    let mut transfer_status = use_signal(|| TransferStatus::Idle);
    let mut clipboard_sync_enabled = use_signal(|| false);
    let mut notifications = use_signal(Vec::<Notification>::new);
    let mut show_send_dialog = use_signal(|| false);
    let mut clipboard_text = use_signal(String::new);
    let mut show_clipboard_dialog = use_signal(|| false);
    let discovery_active = use_signal(|| false);

    // Pairing State
    let pairing_mode = use_signal(|| false);
    let mut pairing_requests = use_signal(Vec::<PairingRequest>::new);
    let mut file_transfer_requests = use_signal(Vec::<FileTransferRequest>::new);

    // The Controller
    let action_tx =
        use_coroutine(
            move |rx: UnboundedReceiver<AppAction>| async move { app_controller(rx).await },
        );

    // Start Init
    use_effect(move || {
        action_tx.send(AppAction::Init);
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
            let pending = get_pending_pairings().lock().unwrap();
            for device in list.iter_mut() {
                if pending.contains(&device.id) {
                    device.is_pending = true;
                }
            }
            drop(pending);

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

            async_std::task::sleep(Duration::from_millis(200)).await;
        }
    });

    let toggle_clipboard_sync = move |_| {
        let current = *clipboard_sync_enabled.read();
        let new_state = !current;
        clipboard_sync_enabled.set(new_state);
        action_tx.send(AppAction::SetClipboardSync(new_state));
        if new_state {
            *get_last_clipboard().lock().unwrap() = get_system_clipboard();
            add_notification("Clipboard Sync", "Sync Started (Trusted Devices)", "📋");
        } else {
            add_notification("Clipboard Sync", "Sync Stopped", "📋");
        }
    };

    let toggle_pairing_mode = move |_| {
        let current = *pairing_mode.read();
        action_tx.send(AppAction::SetPairingMode(!current));
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
                        class: if *active_tab.read() == "devices" { "nav-item active" } else { "nav-item" },
                        onclick: move |_| active_tab.set("devices".to_string()),
                        span { "Devices" }
                        if !devices_list.read().is_empty() {
                            span { class: "nav-badge", "{devices_list.read().len()}" }
                        }
                    }
                    button {
                        class: if *active_tab.read() == "transfers" { "nav-item active" } else { "nav-item" },
                        onclick: move |_| active_tab.set("transfers".to_string()),
                        span { "Transfers" }
                    }
                    button {
                        class: if *active_tab.read() == "clipboard" { "nav-item active" } else { "nav-item" },
                        onclick: move |_| active_tab.set("clipboard".to_string()),
                        span { "Clipboard" }
                        if *clipboard_sync_enabled.read() {
                            span { class: "nav-badge sync", "SYNC" }
                        }
                    }
                    button {
                        class: if *active_tab.read() == "settings" { "nav-item active" } else { "nav-item" },
                        onclick: move |_| active_tab.set("settings".to_string()),
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
                                        active_tab.set("clipboard".to_string());
                                    },
                                    on_browse_files: move |d: DeviceInfo| {
                                        selected_device.set(Some(d));
                                        active_tab.set("files".to_string());
                                    },
                                    on_pair: move |d: DeviceInfo| {
                                         if let Ok(port) = d.port.to_string().parse() {
                                             action_tx.send(AppAction::PairWithDevice { ip: d.ip.clone(), port });
                                         }
                                    },
                                    on_unpair: move |d: DeviceInfo| {
                                        action_tx.send(AppAction::UnpairDevice { fingerprint: "TODO".to_string(), device_id: d.id.clone() });
                                    },
                                    on_forget: move |d: DeviceInfo| {
                                        action_tx.send(AppAction::ForgetDevice { fingerprint: "TODO".to_string(), device_id: d.id.clone() });
                                    },
                                    on_block: move |d: DeviceInfo| {
                                        action_tx.send(AppAction::BlockDevice { fingerprint: "TODO".to_string(), device_id: d.id.clone() });
                                    }
                                }
                            }
                        }
                    }
                }

                // Tab: Files
                if *active_tab.read() == "files" {
                    if let Some(device) = selected_device.read().as_ref() {
                        FileBrowser {
                            device: device.clone(),
                            on_close: move |_| active_tab.set("devices".to_string()),
                        }
                    } else {
                        div { "Please select a device" }
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
                            class: if *clipboard_sync_enabled.read() { "sync-status active" } else { "sync-status" },
                            if *clipboard_sync_enabled.read() {
                                div { class: "sync-icon", "🔄" }
                            } else {
                                div { class: "sync-icon muted", "📋" }
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
                                    type: "checkbox",
                                    checked: "{clipboard_sync_enabled}",
                                    oninput: toggle_clipboard_sync,
                                }
                                span { class: "slider" }
                            }
                        }

                        div {
                            class: "manual-clipboard",
                            h3 { "Manual Send" }
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
                                                let port = device.port;
                                                let text = clipboard_text.read().clone();
                                                if !text.is_empty() {
                                                    action_tx.send(AppAction::SendClipboard { ip, port, text });
                                                    add_notification("Clipboard", "Sending...", "📤");
                                                }
                                            }
                                        },
                                        "Send to {device.name}"
                                    }
                                } else {
                                     button {
                                        class: "primary-button",
                                        disabled: true,
                                        "Select a Device"
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

                        div {
                            class: "info-card",
                            h3 { "🔐 Security" }
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
                                                action_tx.send(AppAction::RejectDevice { fingerprint: req.fingerprint.clone() });
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
