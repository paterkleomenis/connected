use crate::fs_provider::DesktopFilesystemProvider;
use crate::mpris_server::{MprisUpdate, send_mpris_update};
use crate::proximity;
use crate::state::{
    DeviceInfo, FileTransferRequest, PairingRequest, PreviewData, RemoteMedia, SavedDeviceInfo,
    TransferStatus, add_file_transfer_request, add_notification, get_auto_sync_messages,
    get_current_media, get_current_remote_files, get_current_remote_path, get_device_name_setting,
    get_devices_store, get_last_clipboard, get_last_remote_media_device_id, get_last_remote_update,
    get_media_enabled, get_pairing_requests, get_pending_pairings, get_phone_call_log,
    get_phone_conversations, get_phone_data_update, get_phone_messages, get_preview_data,
    get_remote_files_update, get_saved_devices_setting, get_transfer_status, get_update_info,
    mark_calls_synced, mark_contacts_synced, mark_messages_synced, remove_device_from_settings,
    remove_file_transfer_request, save_device_to_settings, set_active_call,
    set_device_name_setting, set_pairing_mode_state, set_phone_call_log, set_phone_contacts,
    set_phone_conversations, set_phone_messages,
};
use crate::utils::{get_hostname, set_system_clipboard};
use connected_core::telephony::{CallAction, TelephonyMessage};
use connected_core::telephony::{CallLogEntry, CallType};
use connected_core::transport::UnpairReason;
use connected_core::update::UpdateChecker;
use connected_core::{
    ConnectedClient, ConnectedEvent, DeviceType, MediaCommand, MediaControlMessage, MediaState,
};
use dioxus::prelude::*;
use futures_util::StreamExt;
#[cfg(target_os = "linux")]
use mpris::PlaybackStatus;
use once_cell::sync::Lazy;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Instant;
use tracing::{debug, error, info, warn};

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
        device_id: String,
    },
    UnpairDevice {
        device_id: String,
    },
    ForgetDevice {
        device_id: String,
    },
    SetPairingMode(bool),
    AcceptFileTransfer {
        transfer_id: String,
    },
    RejectFileTransfer {
        transfer_id: String,
    },
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
    ToggleMediaControl {
        enabled: bool,
        notify: bool,
    },
    SendMediaCommand {
        ip: String,
        port: u16,
        command: MediaCommand,
    },
    ControlRemoteMedia(MediaCommand),
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
    RefreshDiscovery,
    CheckForUpdates,
    PerformUpdate,
}

static PROXIMITY_HANDLE: Lazy<Mutex<Option<proximity::ProximityHandle>>> =
    Lazy::new(|| Mutex::new(None));

type MediaPollStateUpdate = (Option<String>, Option<String>, Option<String>, bool);

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

                    if d.ip != "0.0.0.0" {
                        save_device_to_settings(
                            d.id.clone(),
                            SavedDeviceInfo {
                                name: d.name.clone(),
                                ip: d.ip.clone(),
                                port: d.port,
                                device_type: d.device_type.as_str().to_string(),
                            },
                        );
                    }
                }
                ConnectedEvent::DeviceLost(id) => {
                    let mut store = get_devices_store().lock().unwrap();
                    if let Some(d) = store.remove(&id) {
                        add_notification("Device Lost", &format!("{} disconnected", d.name), "");
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
                            "",
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
                    add_notification("Transfer Complete", &format!("{} finished", filename), "");
                }
                ConnectedEvent::TransferFailed { error, .. } => {
                    *get_transfer_status().lock().unwrap() = TransferStatus::Failed(error.clone());
                    add_notification("Transfer Failed", &error, "");
                }
                ConnectedEvent::ClipboardReceived {
                    content,
                    from_device,
                } => {
                    set_system_clipboard(&content);
                    *get_last_clipboard().lock().unwrap() = content.clone();
                    *get_last_remote_update().lock().unwrap() = Instant::now();
                    add_notification("Clipboard", &format!("Received from {}", from_device), "");
                }
                ConnectedEvent::Error(msg) => {
                    error!("System error: {}", msg);
                }
                ConnectedEvent::PairingRequest {
                    device_name,
                    fingerprint,
                    device_id,
                } => {
                    // Check if already trusted
                    if c_clone.is_device_trusted(&device_id) {
                        info!(
                            "Auto-accepting pairing request from trusted device: {} ({})",
                            device_name, device_id
                        );
                        // Send confirmation directly
                        let c_trust = c_clone.clone();
                        let d_id = device_id.clone();
                        tokio::spawn(async move {
                            // We need the IP to send confirmation. Since we received a request,
                            // the device should be in discovered list (connected source).
                            let devices = c_trust.get_discovered_devices();
                            if let Some(d) = devices.iter().find(|d| d.id == d_id)
                                && let Some(ip) = d.ip_addr()
                                && let Err(e) = c_trust.send_trust_confirmation(ip, d.port).await
                            {
                                warn!("Failed to send trust confirmation: {}", e);
                            }
                        });
                        return;
                    }

                    // Deduplicate requests
                    let mut requests = get_pairing_requests().lock().unwrap();
                    if !requests.iter().any(|r| r.fingerprint == fingerprint) {
                        add_notification(
                            "Pairing Request",
                            &format!("{} wants to connect.", device_name),
                            "",
                        );
                        requests.push(PairingRequest {
                            fingerprint,
                            device_name,
                            device_id,
                        });
                    }
                }
                ConnectedEvent::PairingRejected {
                    device_name,
                    device_id,
                } => {
                    add_notification(
                        "Pairing Rejected",
                        &format!("{} rejected your request.", device_name),
                        "",
                    );
                    get_pending_pairings().lock().unwrap().remove(&device_id);
                }
                ConnectedEvent::PairingModeChanged(enabled) => {
                    info!("Pairing mode changed: {}", enabled);
                    set_pairing_mode_state(enabled);
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

                    if matches!(reason, UnpairReason::Forgotten) {
                        remove_device_from_settings(&device_id);

                        // If device is not discovered (offline), remove it from UI
                        let is_discovered = c_clone
                            .get_discovered_devices()
                            .iter()
                            .any(|d| d.id == device_id);
                        if !is_discovered {
                            let mut store = get_devices_store().lock().unwrap();
                            store.remove(&device_id);
                        }
                    }

                    let reason_str = match reason {
                        UnpairReason::Unpaired => "unpaired from",
                        UnpairReason::Forgotten => "forgotten by",
                    };
                    add_notification(
                        "Device Disconnected",
                        &format!("You were {} {}", reason_str, device_name),
                        "",
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
                        "",
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

	                                // Execute command via MPRIS with manual scan
	                                #[cfg(target_os = "linux")]
	                                let _last_player_identity = last_player_identity.clone();
	                                #[cfg(target_os = "windows")]
	                                let runtime_handle = tokio::runtime::Handle::current();
	                                tokio::task::spawn_blocking(move || {
	                                    #[cfg(target_os = "linux")]
	                                    {
	                                        let last_identity = _last_player_identity.clone();
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
                                                    && *n != "org.mpris.MediaPlayer2.connected"
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
                                                && let Ok(player) =
                                                    Player::new(p_conn, name.clone(), 2000)
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

                                        let target_player = playing_player
                                            .or(preferred_player)
                                            .or(generic_paused)
                                            .or(generic_any);

                                        if let Some(player) = target_player {
                                            // Update last identity

                                            *last_identity.lock().unwrap() =
                                                Some(player.identity().to_string());

                                            info!(
                                                "MPRIS: Controlling player: {}",
                                                player.identity()
                                            );

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
                                                warn!("MPRIS Command Error: {}", e);
                                            }
                                        } else {
                                            warn!("No controllable media player found");
                                        }
	                                    }
	                                    #[cfg(target_os = "windows")]
	                                    {
	                                        use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager;

                                        async fn control_media_windows(
                                            cmd: MediaCommand,
                                        ) -> windows::core::Result<()>
                                        {
                                            let manager: GlobalSystemMediaTransportControlsSessionManager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?.await?;
                                            let session = manager.GetCurrentSession()?;

                                            match cmd {
                                                MediaCommand::Play => {
                                                    session.TryPlayAsync()?.await?;
                                                }
                                                MediaCommand::Pause => {
                                                    session.TryPauseAsync()?.await?;
                                                }
                                                MediaCommand::PlayPause => {
                                                    session.TryTogglePlayPauseAsync()?.await?;
                                                }
                                                MediaCommand::Next => {
                                                    session.TrySkipNextAsync()?.await?;
                                                }
                                                MediaCommand::Previous => {
                                                    session.TrySkipPreviousAsync()?.await?;
                                                }
                                                MediaCommand::Stop => {
                                                    session.TryStopAsync()?.await?;
                                                }
                                                // Volume control is not directly available via SMTC session
                                                _ => {}
                                            }
	                                            Ok(())
	                                        }

	                                        if let Err(e) = runtime_handle.block_on(control_media_windows(cmd))
	                                        {
	                                            warn!("Windows Media Control Error: {}", e);
	                                        }
	                                    }
	                                    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
	                                    {
                                        warn!("Media control not implemented for this OS");
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

                                let state_for_mpris = state.clone();
                                *get_last_remote_media_device_id().lock().unwrap() =
                                    Some(device_id.clone());
                                *get_current_media().lock().unwrap() = Some(RemoteMedia {
                                    state,
                                    source_device_id: device_id,
                                });

                                send_mpris_update(MprisUpdate {
                                    title: state_for_mpris.title,
                                    artist: state_for_mpris.artist,
                                    album: state_for_mpris.album,
                                    playing: state_for_mpris.playing,
                                });
                            }
                        }
                    } else {
                        warn!("IGNORED: Media control is disabled in settings");
                    }
                }
                ConnectedEvent::Telephony {
                    from_device,
                    from_ip,
                    from_port,
                    message,
                } => {
                    info!(
                        "Received telephony event from {}: {:?}",
                        from_device, message
                    );
                    use connected_core::TelephonyMessage;
                    match message {
                        TelephonyMessage::ContactsSyncResponse { contacts } => {
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
                            info!(
                                "MessagesResponse received: thread_id={}, count={}",
                                thread_id,
                                messages.len()
                            );
                            if let Some(latest) =
                                messages.iter().max_by_key(|m| m.timestamp).cloned()
                            {
                                let mut convos = get_phone_conversations().lock().unwrap();
                                if let Some(convo) = convos.iter_mut().find(|c| c.id == thread_id) {
                                    convo.last_message = Some(latest.body.clone());
                                    convo.last_timestamp = latest.timestamp;
                                }
                                convos.sort_by(|a, b| b.last_timestamp.cmp(&a.last_timestamp));
                            }
                            set_phone_messages(thread_id, messages);
                            // Silent load - no notification needed
                        }
                        TelephonyMessage::NewSmsNotification { message } => {
                            info!(
                                "NewSmsNotification received: thread_id={}, from={}",
                                message.thread_id, message.address
                            );
                            let log_preview = {
                                let mut iter = message.body.chars();
                                let mut s: String = iter.by_ref().take(30).collect();
                                if iter.next().is_some() {
                                    s.push_str("...");
                                }
                                s
                            };
                            info!(
                                "NEW SMS NOTIFICATION: from={}, body={}",
                                message.address, log_preview
                            );
                            let sender = message
                                .contact_name
                                .clone()
                                .unwrap_or(message.address.clone());
                            let preview = {
                                let mut iter = message.body.chars();
                                let mut s: String = iter.by_ref().take(50).collect();
                                if iter.next().is_some() {
                                    s.push_str("...");
                                }
                                s
                            };
                            let thread_id = message.thread_id.clone();
                            let msg_timestamp = message.timestamp;
                            let msg_body = message.body.clone();
                            let msg_address = message.address.clone();
                            let msg_contact = message.contact_name.clone();

                            // Add to existing messages for this thread (or create thread)
                            {
                                let mut msgs = get_phone_messages().lock().unwrap();
                                if let Some(thread_msgs) = msgs.get_mut(&thread_id) {
                                    thread_msgs.push(message.clone());
                                } else {
                                    msgs.insert(thread_id.clone(), vec![message.clone()]);
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
                                        id: thread_id.clone(),
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
                                "",
                            );
                            info!("Notification added successfully");

                            if get_auto_sync_messages()
                                && let Ok(ip_addr) = from_ip.parse()
                            {
                                let msg = TelephonyMessage::MessagesRequest {
                                    thread_id: thread_id.clone(),
                                    limit: 200,
                                    before_timestamp: None,
                                };
                                let c = c_clone.clone();
                                tokio::spawn(async move {
                                    let _ = c.send_telephony(ip_addr, from_port, msg).await;
                                });
                            }
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
                                        "",
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
    // Use new_with_bind_all to listen on all interfaces (0.0.0.0)
    // This is crucial for Windows/Linux with multiple interfaces (WSL, Docker, etc.)
    // to ensure we receive traffic regardless of which IP was advertised.
    match ConnectedClient::new_with_bind_all(name.clone(), device_type, 0, None).await {
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
            add_notification("Init Failed", &e.to_string(), "");
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
                /*
                // Don't inject saved devices at startup to avoid showing stale devices
                // Let mDNS/Proximity discovery populate the list with actually active devices
                if let Some(c) = &client {
                    let saved = get_saved_devices_setting();
                    for (device_id, info) in saved {
                        if info.ip == "0.0.0.0" {
                            continue;
                        }
                        let device_type = connected_core::DeviceType::from_str(&info.device_type);
                        let ip = match info.ip.parse() {
                            Ok(ip) => ip,
                            Err(_) => continue,
                        };
                        let _ = c.inject_proximity_device(
                            device_id,
                            info.name,
                            device_type,
                            ip,
                            info.port,
                        );
                    }
                }
                */
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
                            "",
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
                            "",
                        );
                        return;
                    }
                    debug!(
                        "Clipboard send requested to {}:{} (len: {})",
                        ip,
                        port,
                        text.len()
                    );
                    let c = c.clone();
                    tokio::spawn(async move {
                        if let Ok(ip_addr) = ip.parse() {
                            match c.send_clipboard(ip_addr, port, text).await {
                                Ok(()) => {
                                    debug!("Clipboard send succeeded to {}:{}", ip_addr, port);
                                }
                                Err(e) => {
                                    warn!("Clipboard send failed to {}:{}: {}", ip_addr, port, e);
                                }
                            }
                        } else {
                            warn!("Clipboard send skipped: invalid IP {}", ip);
                        }
                    });
                }
            }
            AppAction::BroadcastClipboard { text } => {
                if let Some(c) = &client {
                    debug!("Clipboard broadcast requested (len: {})", text.len());
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
                            "",
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
                                        "",
                                    );
                                    // Remove from pending on success
                                    if let Some(did) = device_id_for_cleanup {
                                        get_pending_pairings().lock().unwrap().remove(&did);
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to send handshake: {}", e);
                                    // Suppress notification if rejected, as ConnectedEvent::PairingRejected handles it
                                    if !e.to_string().to_lowercase().contains("rejected") {
                                        add_notification("Pairing Failed", &e.to_string(), "");
                                    }
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
                            add_notification("Trust Failed", &e.to_string(), "");
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

                            add_notification("Paired", "Device trusted successfully", "");

                            // Send trust confirmation (HandshakeAck) to the other device
                            // This is NOT a new handshake - it confirms we accepted their pairing request
                            let c_clone = c.clone();
                            let did = device_id.clone();
                            tokio::spawn(async move {
                                let devices = c_clone.get_discovered_devices();
                                if let Some(d) = devices.iter().find(|d| d.id == did)
                                    && let Some(ip) = d.ip_addr()
                                    && let Err(e) =
                                        c_clone.send_trust_confirmation(ip, d.port).await
                                {
                                    warn!("Failed to send trust confirmation: {}", e);
                                }
                            });
                        }
                    }
                }
            }
            AppAction::RejectDevice {
                fingerprint,
                device_id,
            } => {
                if let Some(c) = &client {
                    // Remove from pairing requests
                    {
                        let mut requests = get_pairing_requests().lock().unwrap();
                        requests.retain(|r| r.fingerprint != fingerprint);
                    }

                    info!("Rejecting device pairing request: {}", fingerprint);
                    let c = c.clone();
                    let did = device_id.clone();
                    tokio::spawn(async move {
                        if let Err(e) = c.reject_pairing(&did).await {
                            warn!("Failed to send rejection to {}: {}", did, e);
                        }
                    });
                }
            }
            AppAction::UnpairDevice { device_id } => {
                if let Some(c) = &client {
                    let c = c.clone();
                    let did = device_id.clone();
                    tokio::spawn(async move {
                        match c.unpair_device_by_id(&did).await {
                            Err(e) => {
                                error!("Failed to unpair: {}", e);
                                add_notification("Unpair Failed", &e.to_string(), "");
                            }
                            Ok(_) => {
                                // Core emits event, event loop handles UI update
                            }
                        }
                    });
                }
            }
            AppAction::SetPairingMode(enabled) => {
                if let Some(c) = &client {
                    c.set_pairing_mode(enabled);
                }
            }
            AppAction::ForgetDevice { device_id } => {
                if let Some(c) = &client {
                    let c = c.clone();
                    let did = device_id.clone();
                    tokio::spawn(async move {
                        match c.forget_device_by_id(&did).await {
                            Err(e) => {
                                error!("Failed to forget device: {}", e);
                                add_notification("Forget Failed", &e.to_string(), "");
                            }
                            Ok(_) => {
                                // Core emits event, event loop handles UI update
                            }
                        }
                    });
                }
            }
            AppAction::AcceptFileTransfer { transfer_id } => {
                if let Some(c) = &client {
                    // Remove from pending requests
                    remove_file_transfer_request(&transfer_id);
                    if let Err(e) = c.accept_file_transfer(&transfer_id) {
                        error!("Failed to accept file transfer: {}", e);
                        add_notification("Transfer Error", &e.to_string(), "");
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
                            add_notification("Transfer Rejected", "File transfer declined", "");
                        }
                    }
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
                                        "",
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
                                "",
                            );

                            match c
                                .fs_download_file(ip_addr, port, remote_path, local_path.clone())
                                .await
                            {
                                Ok(_) => {
                                    add_notification(
                                        "Download",
                                        &format!("Downloaded {}", filename),
                                        "",
                                    );
                                }
                                Err(e) => {
                                    error!("Failed to download file: {}", e);
                                    add_notification("Download Failed", &e.to_string(), "");
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

                            add_notification("Preview", &format!("Loading {}...", filename), "");

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
                                        add_notification("Preview", "Failed to read file", "");
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to preview file: {}", e);
                                    add_notification("Preview Failed", &e.to_string(), "");
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
            AppAction::ToggleMediaControl { enabled, notify } => {
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

	                                #[cfg(target_os = "windows")]
	                                let runtime_handle = tokio::runtime::Handle::current();

	                                let state_update: Option<MediaPollStateUpdate> =
	                                    tokio::task::spawn_blocking(move || -> Option<MediaPollStateUpdate> {
	                                    #[cfg(target_os = "linux")]
	                                    {
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
                                                    && *n != "org.mpris.MediaPlayer2.connected"
                                            })
	                                            .collect();

	                                        // Find first playing, or just first one
	                                        let mut best_candidate: Option<MediaPollStateUpdate> = None;

	                                        for name in mpris_names {
	                                            // Player::new takes (conn, bus_name, timeout_ms)
	                                            // We must create a new connection for each player because Player::new takes ownership
                                            if let Ok(p_conn) =
                                                Connection::get_private(BusType::Session)
                                                && let Ok(player) =
                                                    Player::new(p_conn, name.clone(), 2000)
                                            {
                                                let _identity = player.identity().to_string();
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

	                                                let candidate: MediaPollStateUpdate =
	                                                    (title, artist, album, playing);

	                                                if playing {
	                                                    // Found a playing one, return immediately
	                                                    return Some(candidate);
                                                } else if best_candidate.is_none() {
                                                    // Keep as fallback
                                                    best_candidate = Some(candidate);
                                                }
                                            }
                                        }

                                        best_candidate
                                    }
	                                    #[cfg(target_os = "windows")]
	                                    {
	                                        use windows::Media::Control::{GlobalSystemMediaTransportControlsSessionManager, GlobalSystemMediaTransportControlsSessionPlaybackStatus};

	                                        async fn get_media_state_windows() -> windows::core::Result<Option<MediaPollStateUpdate>> {
	                                            let manager: GlobalSystemMediaTransportControlsSessionManager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?.await?;
	                                            let session = manager.GetCurrentSession()?;

	                                            let properties: windows::Media::Control::GlobalSystemMediaTransportControlsSessionMediaProperties = session.TryGetMediaPropertiesAsync()?.await?;

                                            let title = properties.Title()?;
                                            let artist = properties.Artist()?;
                                            let album = properties.AlbumTitle()?;

                                            // Timeline is not strictly needed for basic title/artist
                                            // let timeline = session.GetTimelineProperties()?;
                                            let status = session.GetPlaybackInfo()?.PlaybackStatus()?;

                                            let playing = status == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing;

                                            let title_str = if !title.is_empty() { Some(title.to_string()) } else { None };
                                            let artist_str = if !artist.is_empty() { Some(artist.to_string()) } else { None };
                                            let album_str = if !album.is_empty() { Some(album.to_string()) } else { None };

	                                            Ok(Some((title_str, artist_str, album_str, playing)))
	                                        }

	                                        runtime_handle.block_on(get_media_state_windows()).unwrap_or_default()
	                                    }
	                                    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
	                                    {
	                                        None
	                                    }
                                })
                                .await
                                .unwrap_or(None);

                                if let Some((title, artist, album, playing)) = state_update {
                                    let current_title: String = title.clone().unwrap_or_default();

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
                                                    && let Some(ip) = d.ip_addr()
                                                {
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
                            info!("Media poller stopped");
                        });
                        if notify {
                            add_notification("Media Control", "Media control enabled", "");
                        }
                    }
                } else if notify {
                    add_notification("Media Control", "Media control disabled", "");
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
            AppAction::ControlRemoteMedia(command) => {
                if let Some(c) = &client {
                    let device = {
                        let device_id = get_last_remote_media_device_id()
                            .lock()
                            .unwrap()
                            .clone()
                            .unwrap_or_default();
                        if device_id.is_empty() || device_id == "local" || device_id == "unknown" {
                            None
                        } else {
                            let store = get_devices_store().lock().unwrap();
                            store.get(&device_id).cloned()
                        }
                    };

                    if let Some(device) = device {
                        if device.ip == "0.0.0.0" {
                            warn!("Remote media control unavailable in offline mode");
                            return;
                        }
                        let c = c.clone();
                        tokio::spawn(async move {
                            if let Ok(ip_addr) = device.ip.parse() {
                                let _ = c
                                    .send_media_control(
                                        ip_addr,
                                        device.port,
                                        MediaControlMessage::Command(command),
                                    )
                                    .await;
                            }
                        });
                    }
                }
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
                                    add_notification("Phone", &user_msg, "");
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
                                    add_notification("Phone", &user_msg, "");
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
                                }
                                Err(e) => {
                                    error!("Failed to request call log to {}:{}: {}", ip, port, e);
                                    let user_msg = if e.to_string().contains("timed out") {
                                        "Connection timed out. Is the device online?".to_string()
                                    } else {
                                        format!("Sync failed: {}", e)
                                    };
                                    add_notification("Phone", &user_msg, "");
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
                                    add_notification("Phone", &format!("Failed: {}", e), "");
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
                                        "",
                                    );
                                }
                                Err(e) => {
                                    error!("Failed to send SMS: {}", e);
                                    add_notification("Phone", &format!("Failed: {}", e), "");
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
                                        "",
                                    );
                                }
                                Err(e) => {
                                    error!("Failed to initiate call: {}", e);
                                    add_notification("Phone", &format!("Failed: {}", e), "");
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
                                    add_notification("Phone", &format!("Failed: {}", e), "");
                                }
                            }
                        }
                    });
                }
            }
            AppAction::RefreshDiscovery => {
                info!("Refreshed discovery due to adapter state change - Restarting Core");
                if let Some(c) = client.take() {
                    stop_proximity();
                    c.shutdown().await;
                }

                let name = get_device_name_setting().unwrap_or_else(get_hostname);
                client = start_core(name).await;

                if let Some(c) = &client {
                    let saved = get_saved_devices_setting();
                    for (device_id, info) in saved {
                        if info.ip == "0.0.0.0" {
                            continue;
                        }
                        let device_type = connected_core::DeviceType::from_str(&info.device_type)
                            .unwrap_or(connected_core::DeviceType::Unknown);
                        if let Ok(ip) = info.ip.parse() {
                            let _ = c.inject_proximity_device(
                                device_id,
                                info.name,
                                device_type,
                                ip,
                                info.port,
                            );
                        }
                    }
                }
            }
            AppAction::CheckForUpdates => {
                tokio::spawn(async move {
                    let platform = if cfg!(target_os = "linux") {
                        "linux"
                    } else if cfg!(target_os = "windows") {
                        "windows"
                    } else {
                        "unknown"
                    };

                    let current_version = env!("CARGO_PKG_VERSION").to_string();

                    match UpdateChecker::check_for_updates(current_version, platform.to_string())
                        .await
                    {
                        Ok(info) => {
                            *get_update_info().lock().unwrap() = Some(info.clone());
                            if info.has_update {
                                add_notification(
                                    "Update Available",
                                    &format!("Version {} is available", info.latest_version),
                                    "",
                                );
                            } else {
                                // Only show "up to date" if explicitly requested (we might want to distinguish manual vs auto check)
                                // For now, assume manual check if this action is called.
                                add_notification("Updates", "You are up to date", "");
                            }
                        }
                        Err(e) => {
                            error!("Update check failed: {}", e);
                            add_notification("Update Check Failed", &e.to_string(), "");
                        }
                    }
                });
            }
            AppAction::PerformUpdate => {
                let info_opt = get_update_info().lock().unwrap().clone();
                if let Some(info) = info_opt {
                    if let Some(url) = info.download_url {
                        #[cfg(target_os = "windows")]
                        {
                            let latest = info.latest_version.clone();
                            tokio::spawn(async move {
                                // Prefer a real installer flow on Windows rather than pushing the
                                // user to the browser to download + run the MSI manually.
                                if !url.to_lowercase().ends_with(".msi") {
                                    info!("Update URL is not an MSI; opening in browser: {}", url);
                                    let _ = std::process::Command::new("cmd")
                                        .args(["/C", "start", "", &url])
                                        .spawn();
                                    return;
                                }

                                add_notification("Update", "Downloading installer...", "");

                                let mut dest = std::env::temp_dir();
                                dest.push(format!("connected-{}.msi", latest));

                                match connected_core::download_to_file(&url, &dest).await {
                                    Ok(()) => {
                                        let msi = dest.to_string_lossy().to_string();
                                        info!("Launching MSI installer: {}", msi);

                                        let spawn_res = std::process::Command::new("msiexec")
                                            .args(["/i", &msi, "/passive", "/norestart"])
                                            .spawn();

                                        match spawn_res {
                                            Ok(_) => {
                                                // Exit so the installer can replace files cleanly.
                                                std::process::exit(0);
                                            }
                                            Err(e) => {
                                                add_notification(
                                                    "Update Failed",
                                                    &format!("Failed to start installer: {}", e),
                                                    "",
                                                );
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        add_notification(
                                            "Update Failed",
                                            &format!("Download failed: {}", e),
                                            "",
                                        );
                                    }
                                }
                            });
                        }

                        #[cfg(target_os = "linux")]
                        {
                            info!("Opening update URL: {}", url);
                            let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
                        }

                        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
                        {
                            info!("Opening update URL: {}", url);
                        }
                    } else {
                        add_notification("Update", "No download URL found", "");
                    }
                }
            }
        }
    }
}
