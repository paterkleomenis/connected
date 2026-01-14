use crate::fs_provider::DesktopFilesystemProvider;
use crate::proximity;
use crate::state::{
    DeviceInfo, FileTransferRequest, PairingRequest, PreviewData, RemoteMedia, TransferStatus,
    add_file_transfer_request, add_notification, get_current_media, get_current_remote_files,
    get_current_remote_path, get_device_name_setting, get_devices_store, get_last_clipboard,
    get_last_remote_update, get_media_enabled, get_pairing_requests, get_pending_pairings,
    get_phone_call_log, get_phone_conversations, get_phone_data_update, get_phone_messages,
    get_preview_data, get_remote_files_update, get_transfer_status, mark_calls_synced,
    mark_contacts_synced, mark_messages_synced, remove_file_transfer_request, set_active_call,
    set_device_name_setting, set_phone_call_log, set_phone_contacts, set_phone_conversations,
    set_phone_messages,
};
use crate::utils::{get_hostname, set_system_clipboard};
use connected_core::telephony::{CallAction, TelephonyMessage};
use connected_core::telephony::{CallLogEntry, CallType};
use connected_core::transport::UnpairReason;
use connected_core::{
    ConnectedClient, ConnectedEvent, DeviceType, MediaCommand, MediaControlMessage, MediaState,
};
use dioxus::prelude::*;
use futures_util::StreamExt;
use mpris::PlaybackStatus;
use once_cell::sync::Lazy;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Instant;
use tracing::{error, info, warn};

#[derive(Clone, Debug)]
pub enum AppAction {
    Init,
    RenameDevice {
        new_name: String,
    },
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
    DownloadFile {
        ip: String,
        port: u16,
        remote_path: String,
        filename: String,
    },
    PreviewFile {
        ip: String,
        port: u16,
        remote_path: String,
        filename: String,
    },
    GetThumbnail {
        ip: String,
        port: u16,
        path: String,
    },
    ClosePreview,
    ToggleMediaControl(bool),
    SendMediaCommand {
        ip: String,
        port: u16,
        command: MediaCommand,
    },
    ControlLocalMedia(MediaCommand),
    // Telephony actions
    RequestContactsSync {
        ip: String,
        port: u16,
    },
    RequestConversationsSync {
        ip: String,
        port: u16,
    },
    RequestCallLog {
        ip: String,
        port: u16,
        limit: u32,
    },
    RequestMessages {
        ip: String,
        port: u16,
        thread_id: String,
        limit: u32,
    },
    SendSms {
        ip: String,
        port: u16,
        to: String,
        body: String,
    },
    InitiateCall {
        ip: String,
        port: u16,
        number: String,
    },
    SendCallAction {
        ip: String,
        port: u16,
        action: CallAction,
    },
    ClearDevices,
    RefreshDiscovery,
}

static PROXIMITY_HANDLE: Lazy<Mutex<Option<proximity::ProximityHandle>>> =
    Lazy::new(|| Mutex::new(None));

fn start_proximity(client: Arc<ConnectedClient>) {
    let handle = proximity::start(client);
    let mut guard = PROXIMITY_HANDLE.lock().unwrap();
    *guard = handle;
}

fn stop_proximity() {
    if let Some(handle) = PROXIMITY_HANDLE.lock().unwrap().take() {
        handle.stop();
    }
}

fn spawn_event_loop(
    c: Arc<ConnectedClient>,
    mut events: tokio::sync::broadcast::Receiver<ConnectedEvent>,
) {
    let c_clone = c.clone();
    tokio::spawn(async move {
        let last_player_identity = Arc::new(std::sync::Mutex::new(None::<String>));

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
                    store.insert(info.id.clone(), info);
                }
                ConnectedEvent::DeviceLost(id) => {
                    let mut store = get_devices_store().lock().unwrap();
                    if let Some(d) = store.remove(&id) {
                        add_notification("Device Lost", &format!("{} disconnected", d.name), "📴");
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
                        TransferStatus::InProgress { filename, .. } => Some(filename.clone()),
                        _ => None,
                    };
                    if let Some(filename) = current_filename {
                        *status = TransferStatus::InProgress { filename, percent };
                    }
                }
                ConnectedEvent::TransferCompleted { filename, .. } => {
                    *get_transfer_status().lock().unwrap() =
                        TransferStatus::Completed(filename.clone());
                    add_notification("Transfer Complete", &format!("{} finished", filename), "✅");
                }
                ConnectedEvent::TransferFailed { error, .. } => {
                    *get_transfer_status().lock().unwrap() = TransferStatus::Failed(error.clone());
                    add_notification("Transfer Failed", &error, "❌");
                }
                ConnectedEvent::ClipboardReceived {
                    content,
                    from_device,
                } => {
                    set_system_clipboard(&content);
                    *get_last_clipboard().lock().unwrap() = content.clone();
                    *get_last_remote_update().lock().unwrap() = Instant::now();
                    add_notification("Clipboard", &format!("Received from {}", from_device), "📋");
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
                    get_pairing_requests().lock().unwrap().push(PairingRequest {
                        fingerprint,
                        device_name,
                        device_id,
                    });
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
                ConnectedEvent::MediaControl { from_device, event } => {
                    info!("EVENT: Received MediaControl event from {}", from_device);
                    // Only process if media control is enabled locally
                    if *get_media_enabled().lock().unwrap() {
                        match event {
                            MediaControlMessage::Command(cmd) => {
                                info!("COMMAND: Executing {:?} from {}", cmd, from_device);

                                let last_identity = last_player_identity.clone();

                                // Execute command via MPRIS with manual scan
                                tokio::task::spawn_blocking(move || {
                                    use dbus::ffidisp::{BusType, Connection};
                                    use mpris::Player;
                                    use std::rc::Rc;

                                    let conn = match Connection::get_private(BusType::Session) {
                                        Ok(c) => Rc::new(c),
                                        Err(e) => {
                                            warn!("DBus Connection Error: {}", e);
                                            return;
                                        }
                                    };

                                    // ListNames
                                    use dbus::Message;
                                    let msg = Message::new_method_call(
                                        "org.freedesktop.DBus",
                                        "/org/freedesktop/DBus",
                                        "org.freedesktop.DBus",
                                        "ListNames",
                                    )
                                    .unwrap();
                                    let names: Vec<String> =
                                        match conn.send_with_reply_and_block(msg, 5000) {
                                            Ok(reply) => match reply.get1() {
                                                Some(n) => n,
                                                None => return,
                                            },
                                            Err(e) => {
                                                warn!("DBus ListNames Error: {}", e);
                                                return;
                                            }
                                        };

                                    let mpris_names: Vec<&String> = names
                                        .iter()
                                        .filter(|n| {
                                            n.starts_with("org.mpris.MediaPlayer2.")
                                                && !n.contains("playerctld")
                                                && !n.contains("kdeconnect")
                                        })
                                        .collect();

                                    let last_id = last_identity.lock().unwrap().clone();

                                    let mut playing_player: Option<Player> = None;

                                    let mut preferred_player: Option<Player> = None;

                                    let mut generic_paused: Option<Player> = None;

                                    let mut generic_any: Option<Player> = None;

                                    for name in mpris_names {
                                        if let Ok(p_conn) =
                                            Connection::get_private(BusType::Session)
                                        {
                                            if let Ok(player) =
                                                Player::new(p_conn, name.clone().into(), 2000)
                                            {
                                                let identity = player.identity().to_string();

                                                let is_last = last_id.as_ref() == Some(&identity);

                                                match player.get_playback_status() {
                                                    Ok(status) => match status {
                                                        PlaybackStatus::Playing => {
                                                            playing_player = Some(player);

                                                            break;
                                                        }

                                                        PlaybackStatus::Paused => {
                                                            if is_last {
                                                                preferred_player = Some(player);
                                                            } else if generic_paused.is_none() {
                                                                generic_paused = Some(player);
                                                            }
                                                        }

                                                        _ => {
                                                            if is_last {
                                                                preferred_player = Some(player);
                                                            } else if generic_any.is_none() {
                                                                generic_any = Some(player);
                                                            }
                                                        }
                                                    },
                                                    _ => {
                                                        if is_last {
                                                            preferred_player = Some(player);
                                                        } else if generic_any.is_none() {
                                                            generic_any = Some(player);
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    let target_player = playing_player
                                        .or(preferred_player)
                                        .or(generic_paused)
                                        .or(generic_any);

                                    if let Some(player) = target_player {
                                        // Update last identity

                                        *last_identity.lock().unwrap() =
                                            Some(player.identity().to_string());

                                        info!("MPRIS: Controlling player: {}", player.identity());

                                        let res = match cmd {
                                            MediaCommand::Play => player.play(),
                                            MediaCommand::Pause => player.pause(),
                                            MediaCommand::PlayPause => player.play_pause(),
                                            MediaCommand::Next => player.next(),
                                            MediaCommand::Previous => player.previous(),
                                            MediaCommand::Stop => player.stop(),
                                            MediaCommand::VolumeUp => {
                                                let _ = player.set_volume(
                                                    player.get_volume().unwrap_or(0.0) + 0.05,
                                                );
                                                Ok(())
                                            }
                                            MediaCommand::VolumeDown => {
                                                let _ = player.set_volume(
                                                    player.get_volume().unwrap_or(0.0) - 0.05,
                                                );
                                                Ok(())
                                            }
                                        };
                                        if let Err(e) = res {
                                            warn!("MPRIS Command Failed: {}", e);
                                        }
                                    } else {
                                        warn!("MPRIS: No controllable player found");
                                    }
                                });
                            }

                            MediaControlMessage::StateUpdate(state) => {
                                info!(
                                    "STATE: Remote state update from {}: {:?}",
                                    from_device, state.title
                                );

                                // Find device ID by name
                                let device_id = {
                                    let store = get_devices_store().lock().unwrap();
                                    store
                                        .values()
                                        .find(|d| d.name == from_device)
                                        .map(|d| d.id.clone())
                                        .unwrap_or_else(|| "unknown".to_string())
                                };

                                *get_current_media().lock().unwrap() = Some(RemoteMedia {
                                    state,
                                    source_device_id: device_id,
                                });
                            }
                        }
                    } else {
                        warn!("IGNORED: Media control is disabled in settings");
                    }
                }
                ConnectedEvent::Telephony {
                    from_device,
                    from_ip: _,
                    from_port: _,
                    message,
                } => {
                    info!(
                        "Received telephony event from {}: {:?}",
                        from_device, message
                    );
                    use connected_core::TelephonyMessage;
                    match message {
                        TelephonyMessage::ContactsSyncResponse { contacts } => {
                            let count = contacts.len();
                            set_phone_contacts(contacts);
                            mark_contacts_synced();
                            // Silent sync - no notification needed
                        }
                        TelephonyMessage::ConversationsSyncResponse { conversations } => {
                            set_phone_conversations(conversations);
                            mark_messages_synced();
                            // Silent sync - no notification needed
                        }
                        TelephonyMessage::MessagesResponse {
                            thread_id,
                            messages,
                        } => {
                            set_phone_messages(thread_id, messages);
                            // Silent load - no notification needed
                        }
                        TelephonyMessage::NewSmsNotification { message } => {
                            info!(
                                "NEW SMS NOTIFICATION: from={}, body={}",
                                message.address,
                                &message.body[..std::cmp::min(30, message.body.len())]
                            );
                            let sender = message
                                .contact_name
                                .clone()
                                .unwrap_or(message.address.clone());
                            let preview = if message.body.len() > 50 {
                                format!("{}...", &message.body[..50])
                            } else {
                                message.body.clone()
                            };
                            let thread_id = message.thread_id.clone();
                            let msg_timestamp = message.timestamp;
                            let msg_body = message.body.clone();
                            let msg_address = message.address.clone();
                            let msg_contact = message.contact_name.clone();

                            // Add to existing messages for this thread
                            {
                                let mut msgs = get_phone_messages().lock().unwrap();
                                if let Some(thread_msgs) = msgs.get_mut(&thread_id) {
                                    thread_msgs.push(message);
                                }
                            }

                            // Update conversation list with new message
                            {
                                let mut convos = get_phone_conversations().lock().unwrap();

                                // Helper to normalize phone numbers for comparison
                                let normalize_phone = |s: &str| -> String {
                                    s.chars().filter(|c| c.is_ascii_digit()).collect()
                                };
                                let normalized_address = normalize_phone(&msg_address);

                                // Find conversation by thread_id first, then by phone address
                                let existing_convo = convos.iter_mut().find(|c| {
                                    c.id == thread_id
                                        || c.addresses
                                            .iter()
                                            .any(|a| normalize_phone(a) == normalized_address)
                                });

                                if let Some(convo) = existing_convo {
                                    // Update existing conversation
                                    convo.last_message = Some(msg_body.clone());
                                    convo.last_timestamp = msg_timestamp;
                                    convo.unread_count += 1;
                                } else {
                                    // Create new conversation entry
                                    use connected_core::telephony::Conversation;
                                    let new_convo = Conversation {
                                        id: thread_id,
                                        addresses: vec![msg_address],
                                        contact_names: msg_contact
                                            .map(|n| vec![n])
                                            .unwrap_or_default(),
                                        last_message: Some(msg_body.clone()),
                                        last_timestamp: msg_timestamp,
                                        unread_count: 1,
                                    };
                                    convos.push(new_convo);
                                }
                                // Sort by timestamp descending
                                convos.sort_by(|a, b| b.last_timestamp.cmp(&a.last_timestamp));
                            }

                            *get_phone_data_update().lock().unwrap() = std::time::Instant::now();
                            info!("Adding notification for SMS from: {}", sender);
                            add_notification(
                                "New SMS",
                                &format!("From {}: {}", sender, preview),
                                "💬",
                            );
                            info!("Notification added successfully");
                        }
                        TelephonyMessage::CallLogResponse { entries } => {
                            set_phone_call_log(entries);
                            mark_calls_synced();
                            // Silent sync - no notification needed
                        }
                        TelephonyMessage::ActiveCallUpdate { call } => {
                            use connected_core::telephony::ActiveCallState;

                            if let Some(ref c) = call {
                                let caller = c.contact_name.clone().unwrap_or(c.number.clone());

                                // If call ended, add to call log
                                if c.state == ActiveCallState::Ended {
                                    let entry = CallLogEntry {
                                        id: format!(
                                            "call_{}",
                                            std::time::SystemTime::now()
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .unwrap()
                                                .as_millis()
                                        ),
                                        number: c.number.clone(),
                                        contact_name: c.contact_name.clone(),
                                        call_type: if c.is_incoming {
                                            CallType::Incoming
                                        } else {
                                            CallType::Outgoing
                                        },
                                        timestamp: std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap()
                                            .as_millis()
                                            as u64,
                                        duration: c.duration,
                                        is_read: false,
                                    };
                                    // Add to beginning of call log
                                    let mut log = get_phone_call_log().lock().unwrap();
                                    log.insert(0, entry);
                                } else {
                                    add_notification(
                                        "Phone Call",
                                        &format!("{:?} call from {}", c.state, caller),
                                        "📞",
                                    );
                                }
                            }
                            // Store the active call state
                            set_active_call(call.clone());
                        }
                        _ => {
                            // Other telephony messages (requests, etc.)
                        }
                    }
                }
            }
        }
    });
}

async fn start_core(name: String) -> Option<Arc<ConnectedClient>> {
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

            // Spawn event loop
            spawn_event_loop(c.clone(), c.subscribe());

            // Start proximity discovery (platform-specific)
            start_proximity(c.clone());

            Some(c)
        }
        Err(e) => {
            error!("Failed to init: {}", e);
            add_notification("Init Failed", &e.to_string(), "❌");
            None
        }
    }
}

pub async fn app_controller(mut rx: UnboundedReceiver<AppAction>) {
    let mut client: Option<Arc<ConnectedClient>> = None;

    while let Some(action) = rx.next().await {
        match action {
            AppAction::Init => {
                if client.is_some() {
                    continue;
                }
                let name = get_device_name_setting().unwrap_or_else(get_hostname);
                client = start_core(name).await;
            }
            AppAction::RenameDevice { new_name } => {
                if let Some(c) = client.take() {
                    stop_proximity();
                    c.shutdown().await;
                }
                set_device_name_setting(new_name.clone());
                client = start_core(new_name).await;
            }
            AppAction::SendFile { ip, port, path } => {
                if let Some(c) = &client {
                    if ip == "0.0.0.0" {
                        add_notification(
                            "Offline Mode",
                            "Offline transfer not supported. Connect to same network.",
                            "⚠️",
                        );
                        return;
                    }
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
                    if ip == "0.0.0.0" {
                        add_notification(
                            "Offline Mode",
                            "Offline transfer not supported. Connect to same network.",
                            "⚠️",
                        );
                        return;
                    }
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
                                    info!(
                                        "Clipboard broadcast sent to 0 devices (no trusted peers found)"
                                    );
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

                    if ip == "0.0.0.0" {
                        add_notification(
                            "Offline Mode",
                            "Offline pairing not yet supported on Linux. Please connect devices to the same network or use Hotspot.",
                            "⚠️",
                        );
                        warn!("Offline pairing requested but not implemented for Linux");
                        return;
                    }

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
                    match c.trust_device(fingerprint.clone(), Some(device_id.clone()), name.clone())
                    {
                        Err(e) => {
                            error!("Failed to trust: {}", e);
                            add_notification("Trust Failed", &e.to_string(), "❌");
                        }
                        _ => {
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
            }
            AppAction::RejectDevice { fingerprint } => {
                if let Some(c) = &client {
                    // Remove from pairing requests
                    {
                        let mut requests = get_pairing_requests().lock().unwrap();
                        requests.retain(|r| r.fingerprint != fingerprint);
                    }
                    match c.block_device(fingerprint) {
                        Err(e) => {
                            error!("Failed to block device: {}", e);
                        }
                        _ => {
                            add_notification("Blocked", "Device has been blocked", "🚫");
                        }
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

                    match c.unpair_device_by_id(&device_id) {
                        Err(e) => {
                            error!("Failed to unpair: {}", e);
                            add_notification("Unpair Failed", &e.to_string(), "❌");
                        }
                        _ => {
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

                    match c.forget_device(&fingerprint) {
                        Err(e) => {
                            error!("Failed to forget device: {}", e);
                            add_notification("Forget Failed", &e.to_string(), "❌");
                        }
                        _ => {
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

                    match c.block_device(fingerprint) {
                        Err(e) => {
                            error!("Failed to block device: {}", e);
                            add_notification("Block Failed", &e.to_string(), "❌");
                        }
                        _ => {
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
                    match c.reject_file_transfer(&transfer_id) {
                        Err(e) => {
                            error!("Failed to reject file transfer: {}", e);
                        }
                        _ => {
                            add_notification("Transfer Rejected", "File transfer declined", "🚫");
                        }
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
            AppAction::DownloadFile {
                ip,
                port,
                remote_path,
                filename,
            } => {
                if let Some(c) = &client {
                    let c = c.clone();
                    let ip_str = ip.clone();
                    tokio::spawn(async move {
                        if let Ok(ip_addr) = ip_str.parse() {
                            let download_dir =
                                dirs::download_dir().unwrap_or_else(|| PathBuf::from("."));
                            let local_path = download_dir.join(&filename);

                            add_notification(
                                "Download",
                                &format!("Downloading {}...", filename),
                                "📥",
                            );

                            match c
                                .fs_download_file(ip_addr, port, remote_path, local_path.clone())
                                .await
                            {
                                Ok(_) => {
                                    add_notification(
                                        "Download",
                                        &format!("Downloaded {}", filename),
                                        "✅",
                                    );
                                }
                                Err(e) => {
                                    error!("Failed to download file: {}", e);
                                    add_notification("Download Failed", &e.to_string(), "❌");
                                }
                            }
                        }
                    });
                }
            }
            AppAction::PreviewFile {
                ip,
                port,
                remote_path,
                filename,
            } => {
                if let Some(c) = &client {
                    let c = c.clone();
                    let ip_str = ip.clone();
                    tokio::spawn(async move {
                        if let Ok(ip_addr) = ip_str.parse() {
                            let temp_dir = std::env::temp_dir();
                            let local_path = temp_dir.join(format!("preview_{}", filename));

                            add_notification("Preview", &format!("Loading {}...", filename), "👁️");

                            match c
                                .fs_download_file(ip_addr, port, remote_path, local_path.clone())
                                .await
                            {
                                Ok(_) => {
                                    // Read file content
                                    if let Ok(data) = tokio::fs::read(&local_path).await {
                                        let mime = mime_guess::from_path(&filename)
                                            .first_or_octet_stream()
                                            .to_string();

                                        *get_preview_data().lock().unwrap() = Some(PreviewData {
                                            filename,
                                            mime_type: mime,
                                            data,
                                        });
                                        // Clean up temp file
                                        let _ = tokio::fs::remove_file(local_path).await;
                                    } else {
                                        add_notification("Preview", "Failed to read file", "❌");
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to preview file: {}", e);
                                    add_notification("Preview Failed", &e.to_string(), "❌");
                                }
                            }
                        }
                    });
                }
            }
            AppAction::GetThumbnail { ip, port, path } => {
                if let Some(c) = &client {
                    let c = c.clone();
                    let ip_str = ip.clone();
                    tokio::spawn(async move {
                        if let Ok(ip_addr) = ip_str.parse() {
                            // Don't fetch if we already have it (though controller logic usually implies intent to fetch)
                            // But strictly speaking, the UI might check before sending action.
                            // Here we just fetch.
                            match c.fs_get_thumbnail(ip_addr, port, path.clone()).await {
                                Ok(data) => {
                                    if !data.is_empty() {
                                        use crate::state::{get_thumbnails, get_thumbnails_update};
                                        get_thumbnails().lock().unwrap().insert(path, data);
                                        *get_thumbnails_update().lock().unwrap() =
                                            std::time::Instant::now();
                                    }
                                }
                                Err(e) => {
                                    warn!("Failed to get thumbnail for {}: {}", path, e);
                                }
                            }
                        }
                    });
                }
            }
            AppAction::ClosePreview => {
                *get_preview_data().lock().unwrap() = None;
            }
            AppAction::ToggleMediaControl(enabled) => {
                *get_media_enabled().lock().unwrap() = enabled;
                if enabled {
                    info!("Media Poller Started");
                    if let Some(c) = &client {
                        let c = c.clone();
                        // Start MPRIS poller
                        tokio::spawn(async move {
                            let mut interval =
                                tokio::time::interval(std::time::Duration::from_secs(1));
                            let mut last_title = String::new();
                            let mut last_playing = false;

                            // We need to check the atomic flag in the loop
                            while *get_media_enabled().lock().unwrap() {
                                interval.tick().await;

                                let state_update = tokio::task::spawn_blocking(move || {
                                    // Manual D-Bus scan to bypass broken playerctld
                                    use dbus::ffidisp::{BusType, Connection};
                                    use mpris::Player;
                                    use std::rc::Rc;

                                    // Create a new connection for this iteration
                                    let conn = match Connection::get_private(BusType::Session) {
                                        Ok(c) => Rc::new(c),
                                        Err(e) => {
                                            warn!("DBus Connection Error: {}", e);
                                            return None;
                                        }
                                    };

                                    // Use the connection to list names
                                    use dbus::Message;
                                    let msg = Message::new_method_call(
                                        "org.freedesktop.DBus",
                                        "/org/freedesktop/DBus",
                                        "org.freedesktop.DBus",
                                        "ListNames",
                                    )
                                    .unwrap();

                                    let names: Vec<String> =
                                        match conn.send_with_reply_and_block(msg, 5000) {
                                            Ok(reply) => match reply.get1() {
                                                Some(n) => n,
                                                None => return None,
                                            },
                                            Err(e) => {
                                                warn!("DBus ListNames Error: {}", e);
                                                return None;
                                            }
                                        };

                                    let mpris_names: Vec<&String> = names
                                        .iter()
                                        .filter(|n| {
                                            n.starts_with("org.mpris.MediaPlayer2.")
                                                && !n.contains("playerctld")
                                        })
                                        .collect();

                                    // Find first playing, or just first one
                                    let mut best_candidate: Option<(
                                        Option<String>,
                                        Option<String>,
                                        Option<String>,
                                        bool,
                                    )> = None;

                                    for name in mpris_names {
                                        // Player::new takes (conn, bus_name, timeout_ms)
                                        // We must create a new connection for each player because Player::new takes ownership
                                        if let Ok(p_conn) =
                                            Connection::get_private(BusType::Session)
                                        {
                                            if let Ok(player) =
                                                Player::new(p_conn, name.clone().into(), 2000)
                                            {
                                                let meta = player.get_metadata().ok();
                                                let status = player.get_playback_status().ok();
                                                let playing =
                                                    matches!(status, Some(PlaybackStatus::Playing));

                                                let title = meta
                                                    .as_ref()
                                                    .and_then(|m| m.title().map(|s| s.to_string()));
                                                let artist = meta.as_ref().and_then(|m| {
                                                    m.artists().map(|a| a.join(", "))
                                                });
                                                let album = meta.as_ref().and_then(|m| {
                                                    m.album_name().map(|s| s.to_string())
                                                });

                                                let candidate = (title, artist, album, playing);

                                                if playing {
                                                    // Found a playing one, return immediately
                                                    return Some(candidate);
                                                } else if best_candidate.is_none() {
                                                    // Keep as fallback
                                                    best_candidate = Some(candidate);
                                                }
                                            }
                                        }
                                    }

                                    best_candidate
                                })
                                .await
                                .unwrap_or(None);

                                if let Some((title, artist, album, playing)) = state_update {
                                    let current_title = title.clone().unwrap_or_default();

                                    // Only send update if changed
                                    if current_title != last_title || playing != last_playing {
                                        last_title = current_title;
                                        last_playing = playing;

                                        let state = MediaState {
                                            title,
                                            artist,
                                            album,
                                            playing,
                                        };

                                        // Update local state too
                                        *get_current_media().lock().unwrap() = Some(RemoteMedia {
                                            state: state.clone(),
                                            source_device_id: "local".to_string(),
                                        });

                                        // Broadcast to all trusted peers
                                        let peers = c.get_trusted_peers();
                                        for peer in peers {
                                            if let Some(device_id) = peer.device_id {
                                                // Find IP
                                                let discovered = c.get_discovered_devices();
                                                if let Some(d) =
                                                    discovered.iter().find(|d| d.id == device_id)
                                                {
                                                    if let Some(ip) = d.ip_addr() {
                                                        let _ = c
                                                            .send_media_control(
                                                                ip,
                                                                d.port,
                                                                MediaControlMessage::StateUpdate(
                                                                    state.clone(),
                                                                ),
                                                            )
                                                            .await;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            info!("Media poller stopped");
                        });
                        add_notification("Media Control", "Media control enabled", "🎵");
                    }
                } else {
                    add_notification("Media Control", "Media control disabled", "🔇");
                }
            }
            AppAction::SendMediaCommand { ip, port, command } => {
                if let Some(c) = &client {
                    let c = c.clone();
                    tokio::spawn(async move {
                        if let Ok(ip_addr) = ip.parse() {
                            let _ = c
                                .send_media_control(
                                    ip_addr,
                                    port,
                                    MediaControlMessage::Command(command),
                                )
                                .await;
                        }
                    });
                }
            }
            AppAction::ControlLocalMedia(command) => {
                tokio::task::spawn_blocking(move || {
                    use dbus::ffidisp::{BusType, Connection};
                    use mpris::Player;
                    use std::rc::Rc;

                    let conn = match Connection::get_private(BusType::Session) {
                        Ok(c) => Rc::new(c),
                        Err(e) => {
                            warn!("DBus Connection Error: {}", e);
                            return;
                        }
                    };

                    use dbus::Message;
                    let msg = Message::new_method_call(
                        "org.freedesktop.DBus",
                        "/org/freedesktop/DBus",
                        "org.freedesktop.DBus",
                        "ListNames",
                    )
                    .unwrap();
                    let names: Vec<String> = match conn.send_with_reply_and_block(msg, 5000) {
                        Ok(reply) => match reply.get1() {
                            Some(n) => n,
                            None => return,
                        },
                        Err(e) => {
                            warn!("DBus ListNames Error: {}", e);
                            return;
                        }
                    };

                    let mpris_names: Vec<&String> = names
                        .iter()
                        .filter(|n| {
                            n.starts_with("org.mpris.MediaPlayer2.")
                                && !n.contains("playerctld")
                                && !n.contains("kdeconnect")
                        })
                        .collect();

                    let mut playing_player: Option<Player> = None;
                    let mut paused_player: Option<Player> = None;
                    let mut any_player: Option<Player> = None;

                    for name in mpris_names {
                        if let Ok(p_conn) = Connection::get_private(BusType::Session) {
                            if let Ok(player) = Player::new(p_conn, name.clone().into(), 2000) {
                                match player.get_playback_status() {
                                    Ok(status) => match status {
                                        PlaybackStatus::Playing => {
                                            playing_player = Some(player);
                                            break;
                                        }
                                        PlaybackStatus::Paused => {
                                            if paused_player.is_none() {
                                                paused_player = Some(player);
                                            }
                                        }
                                        _ => {
                                            if any_player.is_none() {
                                                any_player = Some(player);
                                            }
                                        }
                                    },
                                    _ => {
                                        if any_player.is_none() {
                                            any_player = Some(player);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    let target_player = playing_player.or(paused_player).or(any_player);

                    if let Some(player) = target_player {
                        info!(
                            "Local Control: Executing {:?} on {}",
                            command,
                            player.identity()
                        );
                        let res = match command {
                            MediaCommand::Play => player.play(),
                            MediaCommand::Pause => player.pause(),
                            MediaCommand::PlayPause => player.play_pause(),
                            MediaCommand::Next => player.next(),
                            MediaCommand::Previous => player.previous(),
                            MediaCommand::Stop => player.stop(),
                            MediaCommand::VolumeUp => {
                                let _ =
                                    player.set_volume(player.get_volume().unwrap_or(0.0) + 0.05);
                                Ok(())
                            }
                            MediaCommand::VolumeDown => {
                                let _ =
                                    player.set_volume(player.get_volume().unwrap_or(0.0) - 0.05);
                                Ok(())
                            }
                        };
                        if let Err(e) = res {
                            warn!("Local Control Failed: {}", e);
                        }
                    }
                });
            }
            AppAction::RequestContactsSync { ip, port } => {
                if let Some(c) = &client {
                    let c = c.clone();
                    tokio::spawn(async move {
                        if let Ok(ip_addr) = ip.parse() {
                            let msg = TelephonyMessage::ContactsSyncRequest;
                            match c.send_telephony(ip_addr, port, msg).await {
                                Ok(_) => {
                                    info!("Contacts sync request sent to {}:{}", ip, port);
                                    add_notification("Phone", "Requesting contacts...", "👤");
                                }
                                Err(e) => {
                                    error!(
                                        "Failed to request contacts sync to {}:{}: {}",
                                        ip, port, e
                                    );
                                    let user_msg = if e.to_string().contains("timed out") {
                                        "Connection timed out. Is the device online?".to_string()
                                    } else {
                                        format!("Sync failed: {}", e)
                                    };
                                    add_notification("Phone", &user_msg, "❌");
                                }
                            }
                        }
                    });
                }
            }
            AppAction::RequestConversationsSync { ip, port } => {
                if let Some(c) = &client {
                    let c = c.clone();
                    tokio::spawn(async move {
                        if let Ok(ip_addr) = ip.parse() {
                            let msg = TelephonyMessage::ConversationsSyncRequest;
                            match c.send_telephony(ip_addr, port, msg).await {
                                Ok(_) => {
                                    info!("Conversations sync request sent to {}:{}", ip, port);
                                    add_notification("Phone", "Requesting messages...", "💬");
                                }
                                Err(e) => {
                                    error!(
                                        "Failed to request conversations sync to {}:{}: {}",
                                        ip, port, e
                                    );
                                    let user_msg = if e.to_string().contains("timed out") {
                                        "Connection timed out. Is the device online?".to_string()
                                    } else {
                                        format!("Sync failed: {}", e)
                                    };
                                    add_notification("Phone", &user_msg, "❌");
                                }
                            }
                        }
                    });
                }
            }
            AppAction::RequestCallLog { ip, port, limit } => {
                if let Some(c) = &client {
                    let c = c.clone();
                    tokio::spawn(async move {
                        if let Ok(ip_addr) = ip.parse() {
                            let msg = TelephonyMessage::CallLogRequest {
                                limit,
                                before_timestamp: None,
                            };
                            match c.send_telephony(ip_addr, port, msg).await {
                                Ok(_) => {
                                    info!("Call log request sent to {}:{}", ip, port);
                                    add_notification("Phone", "Requesting call log...", "📞");
                                }
                                Err(e) => {
                                    error!("Failed to request call log to {}:{}: {}", ip, port, e);
                                    let user_msg = if e.to_string().contains("timed out") {
                                        "Connection timed out. Is the device online?".to_string()
                                    } else {
                                        format!("Sync failed: {}", e)
                                    };
                                    add_notification("Phone", &user_msg, "❌");
                                }
                            }
                        }
                    });
                }
            }
            AppAction::RequestMessages {
                ip,
                port,
                thread_id,
                limit,
            } => {
                if let Some(c) = &client {
                    let c = c.clone();
                    tokio::spawn(async move {
                        if let Ok(ip_addr) = ip.parse() {
                            let msg = TelephonyMessage::MessagesRequest {
                                thread_id,
                                limit,
                                before_timestamp: None,
                            };
                            match c.send_telephony(ip_addr, port, msg).await {
                                Ok(_) => {
                                    info!("Messages request sent to {}:{}", ip, port);
                                }
                                Err(e) => {
                                    error!("Failed to request messages: {}", e);
                                    add_notification("Phone", &format!("Failed: {}", e), "❌");
                                }
                            }
                        }
                    });
                }
            }
            AppAction::SendSms { ip, port, to, body } => {
                if let Some(c) = &client {
                    let c = c.clone();
                    tokio::spawn(async move {
                        if let Ok(ip_addr) = ip.parse() {
                            let msg = TelephonyMessage::SendSms {
                                to: to.clone(),
                                body: body.clone(),
                            };
                            match c.send_telephony(ip_addr, port, msg).await {
                                Ok(_) => {
                                    info!("SMS send request sent to {}:{}", ip, port);
                                    add_notification(
                                        "Phone",
                                        &format!("Sending SMS to {}...", to),
                                        "📤",
                                    );
                                }
                                Err(e) => {
                                    error!("Failed to send SMS: {}", e);
                                    add_notification("Phone", &format!("Failed: {}", e), "❌");
                                }
                            }
                        }
                    });
                }
            }
            AppAction::InitiateCall { ip, port, number } => {
                if let Some(c) = &client {
                    let c = c.clone();
                    tokio::spawn(async move {
                        if let Ok(ip_addr) = ip.parse() {
                            let msg = TelephonyMessage::InitiateCall {
                                number: number.clone(),
                            };
                            match c.send_telephony(ip_addr, port, msg).await {
                                Ok(_) => {
                                    info!("Call initiation request sent to {}:{}", ip, port);
                                    add_notification(
                                        "Phone",
                                        &format!("Calling {}...", number),
                                        "📞",
                                    );
                                }
                                Err(e) => {
                                    error!("Failed to initiate call: {}", e);
                                    add_notification("Phone", &format!("Failed: {}", e), "❌");
                                }
                            }
                        }
                    });
                }
            }
            AppAction::SendCallAction { ip, port, action } => {
                if let Some(c) = &client {
                    let c = c.clone();
                    tokio::spawn(async move {
                        if let Ok(ip_addr) = ip.parse() {
                            let action_name = format!("{:?}", action);
                            let msg = TelephonyMessage::CallAction { action };
                            match c.send_telephony(ip_addr, port, msg).await {
                                Ok(_) => {
                                    info!("Call action {} sent to {}:{}", action_name, ip, port);
                                }
                                Err(e) => {
                                    error!("Failed to send call action: {}", e);
                                    add_notification("Phone", &format!("Failed: {}", e), "❌");
                                }
                            }
                        }
                    });
                }
            }
            AppAction::ClearDevices => {
                // Clear discovered devices from the store
                let mut store = get_devices_store().lock().unwrap();
                store.retain(|_, d| d.is_trusted); // Keep trusted, clear others? Or clear all non-connected?
                // The prompt says "clear the devives cache from ui".
                // We'll clear non-trusted ones to be safe, or maybe just mark them as offline?
                // If the adapter is OFF, they are definitely offline.
                // Let's clear the whole list to reflect "OFF" state dramatically as requested.
                // But keeping trusted ones might be useful for "reconnect".
                // However, "clear the devices cache" implies removing them.
                store.clear();
                info!("Cleared device cache due to adapter state change");

                // Also stop proximity if running
                stop_proximity();
            }
            AppAction::RefreshDiscovery => {
                if let Some(c) = &client {
                    // Restart proximity
                    stop_proximity();
                    start_proximity(c.clone());

                    // Trigger core discovery refresh if needed (core runs MDNS continuously usually)
                    // But we can force a re-scan if the core supports it, or just rely on new proximity/mdns events.
                    // Re-injecting local MDNS service might be needed if IP changed.
                    // For now, restarting proximity is key for BLE/WiFi-Direct.
                    info!("Refreshed discovery due to adapter state change");
                }
            }
        }
    }
}
