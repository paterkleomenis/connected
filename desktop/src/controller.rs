use crate::state::*;
use crate::utils::set_system_clipboard;
use connected_core::{ConnectedClient, ConnectedEvent, DeviceType};
use dioxus::prelude::*;
use futures_util::StreamExt;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info};

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
    SetPairingMode(bool),
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

                // Use default persistence path (None -> ~/.config/connected)
                match ConnectedClient::new(name.clone(), DeviceType::Linux, 0, None).await {
                    Ok(c) => {
                        info!("Core initialized");

                        client = Some(c.clone());

                        // Subscribe to events
                        let mut events = c.subscribe();
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
                        if let Err(e) = c.broadcast_clipboard(text).await {
                            error!("Broadcast failed: {}", e);
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
                    let store = get_devices_store().lock().unwrap();
                    if let Some(d) = store.values().find(|d| d.ip == ip && d.port == port) {
                        get_pending_pairings().lock().unwrap().insert(d.id.clone());
                    }
                    drop(store);

                    tokio::spawn(async move {
                        if let Ok(ip_addr) = ip.parse() {
                            // Enable pairing mode temporarily? Or assume user enabled it?
                            // For initiating, we don't necessarily need to be in pairing mode unless
                            // the other side requires MUTUAL pairing mode.
                            // But good practice: enable pairing mode locally too.
                            c.set_pairing_mode(true);

                            if let Err(e) = c.send_handshake(ip_addr, port).await {
                                error!("Failed to send handshake: {}", e);
                                add_notification("Pairing Failed", &e.to_string(), "❌");
                            } else {
                                add_notification("Pairing", "Request sent...", "🔗");
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
                    if let Err(e) = c.trust_device(fingerprint, Some(device_id.clone()), name) {
                        error!("Failed to trust: {}", e);
                    } else {
                        // Force refresh of devices to update trust status
                        add_notification("Paired", "Device trusted successfully", "✅");

                        // Send handshake back to confirm pairing to the other device
                        let c_clone = c.clone();
                        let did = device_id.clone();
                        tokio::spawn(async move {
                            let devices = c_clone.get_discovered_devices();
                            if let Some(d) = devices.iter().find(|d| d.id == did) {
                                if let Some(ip) = d.ip_addr() {
                                    let _ = c_clone.send_handshake(ip, d.port).await;
                                }
                            }
                        });
                    }
                }
            }
            AppAction::RejectDevice { fingerprint } => {
                if let Some(c) = &client {
                    let _ = c.block_device(fingerprint);
                }
            }
            AppAction::UnpairDevice {
                fingerprint,
                device_id,
            } => {
                if let Some(c) = &client {
                    // If fingerprint is "TODO", look it up from device_id
                    let fingerprint = if fingerprint == "TODO" {
                        // Try to find it in key_store (known peers) by device_id?
                        // KeyStore::get_trusted_peers() returns PeerInfo which has device_id.
                        let peers = c.get_trusted_peers();
                        peers
                            .iter()
                            .find(|p| p.device_id.as_deref() == Some(&device_id))
                            .map(|p| p.fingerprint.clone())
                            .unwrap_or(fingerprint)
                    } else {
                        fingerprint
                    };

                    if let Err(e) = c.remove_trusted_peer(&fingerprint) {
                        error!("Failed to remove peer: {}", e);
                        add_notification("Unpair Failed", &e.to_string(), "❌");
                    } else {
                        // Update local store immediately to reflect change
                        let mut store = get_devices_store().lock().unwrap();
                        if let Some(d) = store.get_mut(&device_id) {
                            d.is_trusted = false;
                        }
                        add_notification("Unpaired", "Device removed from trusted list", "💔");
                    }
                }
            }
            AppAction::SetPairingMode(enabled) => {
                if let Some(c) = &client {
                    c.set_pairing_mode(enabled);
                }
            }
        }
    }
}
