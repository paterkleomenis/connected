use crate::fs_provider::DesktopFilesystemProvider;
use crate::state::*;
use crate::utils::set_system_clipboard;
use connected_core::transport::UnpairReason;
use connected_core::{ConnectedClient, ConnectedEvent, DeviceType};
use dioxus::prelude::*;
use futures_util::StreamExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tracing::{error, info, warn};

#[derive(Clone, Debug)]
pub enum AppAction {
    Init,
    SendFile {
        ip: String,
        port: u16,
        path: String,
    },
    SendClipboard {
        ip: String,
        port: u16,
        text: String,
    },
    BroadcastClipboard {
        text: String,
    },
    SetClipboardSync(bool),
    PairWithDevice {
        ip: String,
        port: u16,
    },
    TrustDevice {
        fingerprint: String,
        name: String,
        device_id: String,
    },
    RejectDevice {
        fingerprint: String,
    },
    UnpairDevice {
        fingerprint: String,
        device_id: String,
    },
    ForgetDevice {
        fingerprint: String,
        device_id: String,
    },
    BlockDevice {
        fingerprint: String,
        device_id: String,
    },
    SetPairingMode(bool),
    AcceptFileTransfer {
        transfer_id: String,
    },
    RejectFileTransfer {
        transfer_id: String,
    },
    SetAutoAcceptFiles(bool),
    ListRemoteFiles {
        ip: String,
        port: u16,
        path: String,
    },
}

pub async fn app_controller(mut rx: UnboundedReceiver<AppAction>) {
    let mut client: Option<Arc<ConnectedClient>> = None;

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

                // Detect device type based on OS
                let device_type = if cfg!(target_os = "linux") {
                    DeviceType::Linux
                } else if cfg!(target_os = "windows") {
                    DeviceType::Windows
                } else if cfg!(target_os = "macos") {
                    DeviceType::MacOS
                } else {
                    DeviceType::Unknown
                };

                // Use default persistence path (None -> ~/.config/connected)
                match ConnectedClient::new(name.clone(), device_type, 0, None).await {
                    Ok(c) => {
                        info!("Core initialized");

                        // Register Filesystem Provider
                        c.register_filesystem_provider(Box::new(DesktopFilesystemProvider::new()));

                        client = Some(c.clone());

                        // Subscribe to events
                        let events = c.subscribe();
                        let mut events = events;
                        let c_clone = c.clone();

                        // Spawn event loop
                        tokio::spawn(async move {
                            while let Ok(event) = events.recv().await {
                                match event {
                                    ConnectedEvent::DeviceFound(d) => {
                                        let mut info: DeviceInfo = d.clone().into();
                                        info.is_trusted = c_clone.is_device_trusted(&d.id);

                                        // If trusted, remove from pending
                                        if info.is_trusted {
                                            get_pending_pairings().lock().unwrap().remove(&info.id);
                                        }

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
                                            (bytes_transferred as f32 / total_size as f32) * 100.0
                                        } else {
                                            0.0
                                        };
                                        let mut status = get_transfer_status().lock().unwrap();
                                        let current_filename = match &*status {
                                            TransferStatus::Starting(f) => Some(f.clone()),
                                            TransferStatus::InProgress { filename, .. } => {
                                                Some(filename.clone())
                                            }
                                            _ => None,
                                        };
                                        if let Some(filename) = current_filename {
                                            *status =
                                                TransferStatus::InProgress { filename, percent };
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
                                        set_system_clipboard(&content);
                                        *get_last_clipboard().lock().unwrap() = content.clone();
                                        *get_last_remote_update().lock().unwrap() = Instant::now();
                                        add_notification(
                                            "Clipboard",
                                            &format!("Received from {}", from_device),
                                            "📋",
                                        );
                                    }
                                    ConnectedEvent::Error(msg) => {
                                        error!("System error: {}", msg);
                                    }
                                    ConnectedEvent::PairingRequest {
                                        device_name,
                                        fingerprint,
                                        device_id,
                                    } => {
                                        add_notification(
                                            "Pairing Request",
                                            &format!("{} wants to connect.", device_name),
                                            "🔐",
                                        );
                                        get_pairing_requests().lock().unwrap().push(
                                            PairingRequest {
                                                fingerprint,
                                                device_name,
                                                device_id,
                                            },
                                        );
                                    }
                                    ConnectedEvent::PairingModeChanged(enabled) => {
                                        info!("Pairing mode changed: {}", enabled);
                                    }
                                    ConnectedEvent::DeviceUnpaired {
                                        device_id,
                                        device_name,
                                        reason,
                                    } => {
                                        // Update local store - device is no longer trusted
                                        {
                                            let mut store = get_devices_store().lock().unwrap();
                                            if let Some(d) = store.get_mut(&device_id) {
                                                d.is_trusted = false;
                                            }
                                        }

                                        let reason_str = match reason {
                                            UnpairReason::Unpaired => "unpaired from",
                                            UnpairReason::Forgotten => "forgotten by",
                                            UnpairReason::Blocked => "blocked by",
                                        };
                                        add_notification(
                                            "Device Disconnected",
                                            &format!("You were {} {}", reason_str, device_name),
                                            "💔",
                                        );
                                    }
                                    ConnectedEvent::TransferRequest {
                                        id,
                                        filename,
                                        size,
                                        from_device,
                                        from_fingerprint,
                                    } => {
                                        add_notification(
                                            "File Transfer Request",
                                            &format!(
                                                "{} wants to send {} ({} bytes)",
                                                from_device, filename, size
                                            ),
                                            "📁",
                                        );
                                        // Store the request for UI to display
                                        add_file_transfer_request(FileTransferRequest {
                                            id: id.clone(),
                                            filename,
                                            size,
                                            from_device,
                                            from_fingerprint,
                                            timestamp: Instant::now(),
                                        });
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
            AppAction::BroadcastClipboard { text } => {
                if let Some(c) = &client {
                    let c = c.clone();
                    tokio::spawn(async move {
                        match c.broadcast_clipboard(text).await {
                            Ok(count) => {
                                if count == 0 {
                                    info!("Clipboard broadcast sent to 0 devices (no trusted peers found)");
                                } else {
                                    info!("Clipboard broadcast sent to {} devices", count);
                                }
                            }
                            Err(e) => {
                                error!("Broadcast failed: {}", e);
                            }
                        }
                    });
                }
            }
            AppAction::SetClipboardSync(_enabled) => {
                // Handled in main loop for state sync, here we just ack
            }
            AppAction::PairWithDevice { ip, port } => {
                if let Some(c) = &client {
                    let c = c.clone();

                    // Find device ID by IP/Port to add to pending
                    let device_id = {
                        let store = get_devices_store().lock().unwrap();
                        store
                            .values()
                            .find(|d| d.ip == ip && d.port == port)
                            .map(|d| d.id.clone())
                    };

                    if let Some(did) = device_id.clone() {
                        get_pending_pairings().lock().unwrap().insert(did);
                    }

                    let ip_clone = ip.clone();
                    let device_id_for_cleanup = device_id;

                    tokio::spawn(async move {
                        if let Ok(ip_addr) = ip_clone.parse() {
                            // send_handshake now automatically enables pairing mode with timeout
                            match c.send_handshake(ip_addr, port).await {
                                Ok(()) => {
                                    add_notification(
                                        "Paired",
                                        "Successfully paired with device",
                                        "✅",
                                    );
                                    // Remove from pending on success
                                    if let Some(did) = device_id_for_cleanup {
                                        get_pending_pairings().lock().unwrap().remove(&did);
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to send handshake: {}", e);
                                    add_notification("Pairing Failed", &e.to_string(), "❌");
                                    // Remove from pending on failure
                                    if let Some(did) = device_id_for_cleanup {
                                        get_pending_pairings().lock().unwrap().remove(&did);
                                    }
                                }
                            }
                        }
                    });
                }
            }
            AppAction::TrustDevice {
                fingerprint,
                name,
                device_id,
            } => {
                if let Some(c) = &client {
                    if let Err(e) =
                        c.trust_device(fingerprint.clone(), Some(device_id.clone()), name.clone())
                    {
                        error!("Failed to trust: {}", e);
                        add_notification("Trust Failed", &e.to_string(), "❌");
                    } else {
                        // Remove from pairing requests
                        {
                            let mut requests = get_pairing_requests().lock().unwrap();
                            requests.retain(|r| r.fingerprint != fingerprint);
                        }

                        // Update device trust status in store
                        {
                            let mut store = get_devices_store().lock().unwrap();
                            if let Some(d) = store.get_mut(&device_id) {
                                d.is_trusted = true;
                            }
                        }

                        add_notification("Paired", "Device trusted successfully", "✅");

                        // Send trust confirmation (HandshakeAck) to the other device
                        // This is NOT a new handshake - it confirms we accepted their pairing request
                        let c_clone = c.clone();
                        let did = device_id.clone();
                        tokio::spawn(async move {
                            let devices = c_clone.get_discovered_devices();
                            if let Some(d) = devices.iter().find(|d| d.id == did) {
                                if let Some(ip) = d.ip_addr() {
                                    if let Err(e) =
                                        c_clone.send_trust_confirmation(ip, d.port).await
                                    {
                                        warn!("Failed to send trust confirmation: {}", e);
                                    }
                                }
                            }
                        });
                    }
                }
            }
            AppAction::RejectDevice { fingerprint } => {
                if let Some(c) = &client {
                    // Remove from pairing requests
                    {
                        let mut requests = get_pairing_requests().lock().unwrap();
                        requests.retain(|r| r.fingerprint != fingerprint);
                    }
                    if let Err(e) = c.block_device(fingerprint) {
                        error!("Failed to block device: {}", e);
                    } else {
                        add_notification("Blocked", "Device has been blocked", "🚫");
                    }
                }
            }
            AppAction::UnpairDevice {
                fingerprint: _,
                device_id,
            } => {
                if let Some(c) = &client {
                    // Unpair = disconnect and remove from UI, but keep backend trust (can auto-reconnect later)

                    // Get device info before unpairing for notification
                    let device_info = {
                        let store = get_devices_store().lock().unwrap();
                        store.get(&device_id).cloned()
                    };

                    if let Err(e) = c.unpair_device_by_id(&device_id) {
                        error!("Failed to unpair: {}", e);
                        add_notification("Unpair Failed", &e.to_string(), "❌");
                    } else {
                        // Remove from UI store so it shows as disconnected
                        {
                            let mut store = get_devices_store().lock().unwrap();
                            if let Some(d) = store.get_mut(&device_id) {
                                d.is_trusted = false;
                            }
                        }

                        // Notify the other device so they also update their UI
                        // Using UnpairReason::Unpaired which preserves backend trust
                        if let Some(info) = device_info {
                            let c_clone = c.clone();
                            tokio::spawn(async move {
                                if let Ok(ip) = info.ip.parse() {
                                    let _ = c_clone
                                        .send_unpair_notification(
                                            ip,
                                            info.port,
                                            UnpairReason::Unpaired,
                                        )
                                        .await;
                                }
                            });
                        }

                        add_notification(
                            "Disconnected",
                            "Device disconnected - can reconnect anytime",
                            "💔",
                        );
                    }
                }
            }
            AppAction::SetPairingMode(enabled) => {
                if let Some(c) = &client {
                    c.set_pairing_mode(enabled);
                }
            }
            AppAction::ForgetDevice {
                fingerprint,
                device_id,
            } => {
                if let Some(c) = &client {
                    // If fingerprint is "TODO", look it up from device_id
                    let fingerprint = if fingerprint == "TODO" {
                        let peers = c.get_trusted_peers();
                        peers
                            .iter()
                            .find(|p| p.device_id.as_deref() == Some(&device_id))
                            .map(|p| p.fingerprint.clone())
                            .unwrap_or(fingerprint)
                    } else {
                        fingerprint
                    };

                    // Get device info before forgetting for notification
                    let device_info = {
                        let store = get_devices_store().lock().unwrap();
                        store.get(&device_id).cloned()
                    };

                    if let Err(e) = c.forget_device(&fingerprint) {
                        error!("Failed to forget device: {}", e);
                        add_notification("Forget Failed", &e.to_string(), "❌");
                    } else {
                        // Update local store to reflect change
                        {
                            let mut store = get_devices_store().lock().unwrap();
                            if let Some(d) = store.get_mut(&device_id) {
                                d.is_trusted = false;
                            }
                        }

                        // Notify the other device that we forgot them
                        if let Some(info) = device_info {
                            let c_clone = c.clone();
                            tokio::spawn(async move {
                                if let Ok(ip) = info.ip.parse() {
                                    let _ = c_clone
                                        .send_unpair_notification(
                                            ip,
                                            info.port,
                                            UnpairReason::Forgotten,
                                        )
                                        .await;
                                }
                            });
                        }

                        add_notification(
                            "Device Forgotten",
                            "Device will require re-pairing approval",
                            "🔄",
                        );
                    }
                }
            }
            AppAction::BlockDevice {
                fingerprint,
                device_id,
            } => {
                if let Some(c) = &client {
                    // If fingerprint is "TODO", look it up from device_id
                    let fingerprint = if fingerprint == "TODO" {
                        let peers = c.get_trusted_peers();
                        peers
                            .iter()
                            .find(|p| p.device_id.as_deref() == Some(&device_id))
                            .map(|p| p.fingerprint.clone())
                            .unwrap_or(fingerprint)
                    } else {
                        fingerprint
                    };

                    // Get device info before blocking for notification
                    let device_info = {
                        let store = get_devices_store().lock().unwrap();
                        store.get(&device_id).cloned()
                    };

                    if let Err(e) = c.block_device(fingerprint) {
                        error!("Failed to block device: {}", e);
                        add_notification("Block Failed", &e.to_string(), "❌");
                    } else {
                        // Remove from local store
                        {
                            let mut store = get_devices_store().lock().unwrap();
                            store.remove(&device_id);
                        }

                        // Notify the other device that we blocked them
                        if let Some(info) = device_info {
                            let c_clone = c.clone();
                            tokio::spawn(async move {
                                if let Ok(ip) = info.ip.parse() {
                                    let _ = c_clone
                                        .send_unpair_notification(
                                            ip,
                                            info.port,
                                            UnpairReason::Blocked,
                                        )
                                        .await;
                                }
                            });
                        }

                        add_notification(
                            "Device Blocked",
                            "Device will no longer be able to connect",
                            "🚫",
                        );
                    }
                }
            }
            AppAction::AcceptFileTransfer { transfer_id } => {
                if let Some(c) = &client {
                    // Remove from pending requests
                    remove_file_transfer_request(&transfer_id);
                    if let Err(e) = c.accept_file_transfer(&transfer_id) {
                        error!("Failed to accept file transfer: {}", e);
                        add_notification("Transfer Error", &e.to_string(), "❌");
                    }
                }
            }
            AppAction::RejectFileTransfer { transfer_id } => {
                if let Some(c) = &client {
                    // Remove from pending requests
                    remove_file_transfer_request(&transfer_id);
                    if let Err(e) = c.reject_file_transfer(&transfer_id) {
                        error!("Failed to reject file transfer: {}", e);
                    } else {
                        add_notification("Transfer Rejected", "File transfer declined", "🚫");
                    }
                }
            }
            AppAction::SetAutoAcceptFiles(enabled) => {
                if let Some(c) = &client {
                    c.set_auto_accept_files(enabled);
                    let msg = if enabled {
                        "Auto-accept enabled"
                    } else {
                        "Auto-accept disabled"
                    };
                    add_notification("Settings", msg, "⚙️");
                }
            }
            AppAction::ListRemoteFiles { ip, port, path } => {
                if let Some(c) = &client {
                    let c = c.clone();
                    let ip_str = ip.clone();
                    tokio::spawn(async move {
                        if let Ok(ip_addr) = ip_str.parse() {
                            match c.fs_list_dir(ip_addr, port, path.clone()).await {
                                Ok(entries) => {
                                    *get_current_remote_files().lock().unwrap() = Some(entries);
                                    *get_current_remote_path().lock().unwrap() = path;
                                    *get_remote_files_update().lock().unwrap() = Instant::now();
                                }
                                Err(e) => {
                                    error!("Failed to list remote files: {}", e);
                                    add_notification(
                                        "File Browser",
                                        &format!("Failed to list: {}", e),
                                        "❌",
                                    );
                                }
                            }
                        }
                    });
                }
            }
        }
    }
}
