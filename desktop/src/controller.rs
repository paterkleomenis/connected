use crate::fs_provider::DesktopFilesystemProvider;
use crate::mpris_server::{MprisUpdate, send_mpris_update};
use std::process::Command;

/// Open a local media file in a desktop media player. Tries common players and
/// falls back to the OS default handler. Returns true if a player was launched.
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
fn open_in_media_player(path: &std::path::Path) -> bool {
    let candidates: &[(&str, &[&str])] = if cfg!(target_os = "windows") {
        &[("vlc", &[]), ("mpv", &[]), ("cmd", &["/c", "start", ""])]
    } else if cfg!(target_os = "macos") {
        &[("vlc", &[]), ("mpv", &[]), ("open", &[])]
    } else {
        &[
            ("vlc", &[]),
            ("mpv", &[]),
            ("totem", &[]),
            ("smplayer", &[]),
            ("xdg-open", &[]),
            ("gio", &["open"]),
        ]
    };

    for (program, extra) in candidates {
        let mut command = Command::new(program);
        for arg in *extra {
            command.arg(arg);
        }
        command.arg(path);
        match command.spawn() {
            Ok(_) => {
                tracing::info!("Opened media \"{}\" with {}", path.display(), program);
                return true;
            }
            Err(e) => {
                tracing::debug!("Could not launch media player {}: {}", program, e);
            }
        }
    }
    false
}
use crate::state::{
    DeviceInfo, FileTransferRequest, LockOrRecover, PairingRequest, PreviewData, RemoteMedia,
    SavedDeviceInfo, TransferStatus, add_actionable_notification, add_file_transfer_request,
    add_notification, add_open_file_notification, get_active_incoming_transfer_id,
    get_active_outgoing_transfer_id, get_auto_sync_messages, get_autostart_enabled_setting,
    get_clipboard_sync_enabled, get_current_media, get_current_remote_files,
    get_current_remote_path, get_device_name_setting, get_devices_store,
    get_download_directory_setting, get_last_clipboard, get_last_remote_clipboard_content,
    get_last_remote_media_device_id, get_last_remote_update, get_media_enabled,
    get_pairing_mode_state, get_pairing_requests, get_pending_pairings, get_phone_call_log,
    get_phone_conversations, get_phone_data_update, get_phone_messages, get_preview_data,
    get_remote_files_update, get_saved_devices_setting, get_transfer_status,
    is_auto_accept_enabled, mark_calls_synced, mark_contacts_synced, mark_messages_synced,
    remove_device_from_settings, remove_file_transfer_request, remove_transfer_path,
    save_device_to_settings, set_active_call, set_active_incoming_transfer_id,
    set_active_outgoing_transfer_id, set_autostart_enabled_setting, set_device_name_setting,
    set_discovery_active, set_download_directory_setting, set_last_remote_clipboard_content,
    set_pairing_mode_state, set_phone_call_log, set_phone_contacts, set_phone_conversations,
    set_phone_messages, set_sdk_initialized, set_shared_folder_setting, set_transfer_status,
    store_transfer_path,
};
use crate::utils::{get_hostname, get_system_clipboard, set_system_clipboard};
use connected_core::telephony::{CallAction, TelephonyMessage};
use connected_core::telephony::{CallLogEntry, CallType};
#[cfg(not(target_os = "windows"))]
use connected_core::update::UpdateChecker;
#[cfg(target_os = "macos")]
use connected_core::update::install_macos_update;
#[cfg(target_os = "linux")]
use connected_core::update::{
    install_linux_appimage_update, install_linux_flatpak_update, is_installed_via_flatpak,
    is_running_as_appimage,
};
use connected_core::{
    ConnectedClient, ConnectedEvent, DeviceType, MediaCommand, MediaControlMessage, MediaState,
};
#[cfg(target_os = "linux")]
use mpris::PlaybackStatus;
#[cfg(target_os = "linux")]
use once_cell::sync::Lazy;
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedReceiver;
use tracing::{debug, error, info, warn};

/// Cached MPRIS player names to avoid repeated DBus ListNames queries.
/// Cache is invalidated after MPRIS_CACHE_TTL.
#[cfg(target_os = "linux")]
static MPRIS_NAMES_CACHE: Lazy<Mutex<(Vec<String>, Instant)>> =
    Lazy::new(|| Mutex::new((Vec::new(), Instant::now() - Duration::from_secs(60))));

#[cfg(target_os = "linux")]
const MPRIS_CACHE_TTL: Duration = Duration::from_secs(5);

/// Get cached MPRIS player names, refreshing if cache is stale.
#[cfg(target_os = "linux")]
fn get_cached_mpris_names() -> Option<Vec<String>> {
    use dbus::Message;
    use dbus::ffidisp::{BusType, Connection};

    let mut cache = MPRIS_NAMES_CACHE.lock().ok()?;
    let (ref mut names, ref mut last_update) = *cache;

    if last_update.elapsed() < MPRIS_CACHE_TTL && !names.is_empty() {
        return Some(names.clone());
    }

    // Refresh cache
    let conn = Connection::get_private(BusType::Session).ok()?;
    let msg = Message::new_method_call(
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "ListNames",
    )
    .ok()?;

    let all_names: Vec<String> = conn.send_with_reply_and_block(msg, 5000).ok()?.get1()?;

    let mpris_names: Vec<String> = all_names
        .into_iter()
        .filter(|n| {
            n.starts_with("org.mpris.MediaPlayer2.")
                && !n.contains("playerctld")
                && !n.contains("kdeconnect")
                && n != "org.mpris.MediaPlayer2.connected"
        })
        .collect();

    *names = mpris_names.clone();
    *last_update = Instant::now();

    Some(mpris_names)
}

#[derive(Clone, Debug)]
pub enum AppAction {
    Init,
    RenameDevice {
        new_name: String,
    },
    SendFile {
        ip: String,
        port: u16,
        paths: Vec<String>,
    },
    SendClipboard {
        ip: String,
        port: u16,
        text: String,
    },
    SetClipboardSync(bool),
    PairWithDevice {
        ip: String,
        port: u16,
    },
    CancelPairing {
        device_id: String,
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
    SetPairingMode(bool),
    AcceptFileTransfer {
        transfer_id: String,
    },
    RejectFileTransfer {
        transfer_id: String,
    },
    CancelFileTransfer,
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
    RefreshDevices,
    SetDownloadDirectory {
        path: String,
    },
    SetSharedFolder {
        path: String,
    },
    SetAutostart(bool),
    CheckForUpdates,
    PerformUpdate,
    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    PickAndSendFiles,
    Shutdown,
}

type MediaPollStateUpdate = (Option<String>, Option<String>, Option<String>, bool);

fn spawn_clipboard_monitor(client: Arc<ConnectedClient>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;

            if !get_clipboard_sync_enabled() {
                continue;
            }

            // Debounce local->remote propagation right after receiving a remote clipboard update.
            let last_remote_update = *get_last_remote_update().lock_or_recover();
            if last_remote_update.elapsed() < Duration::from_millis(1000) {
                continue;
            }

            let current_clip = tokio::task::spawn_blocking(get_system_clipboard)
                .await
                .unwrap_or_default();
            if current_clip.is_empty() {
                continue;
            }

            let last_clip = get_last_clipboard().lock_or_recover().clone();
            let last_remote_content = get_last_remote_clipboard_content()
                .lock_or_recover()
                .clone();
            if current_clip == last_clip || current_clip == last_remote_content {
                continue;
            }

            debug!(
                "Local clipboard changed. New content length: {}",
                current_clip.len()
            );
            *get_last_clipboard().lock_or_recover() = current_clip.clone();

            match client.broadcast_clipboard(current_clip).await {
                Ok(count) => {
                    if count == 0 {
                        info!("Clipboard broadcast sent to 0 devices (no trusted peers found)");
                    } else {
                        info!("Clipboard broadcast sent to {} devices", count);
                    }
                }
                Err(e) => {
                    error!("Clipboard broadcast failed: {}", e);
                }
            }
        }
    })
}

fn spawn_event_loop(
    c: Arc<ConnectedClient>,
    mut events: tokio::sync::broadcast::Receiver<ConnectedEvent>,
) {
    fn is_cancelled_error(error: &str) -> bool {
        let lower = error.to_ascii_lowercase();
        lower.contains("cancelled") || lower.contains("canceled")
    }

    fn check_disk_space(size: u64) -> Result<(), String> {
        let download_dir = get_download_directory_setting()
            .map(std::path::PathBuf::from)
            .or_else(dirs::download_dir)
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        match fs2::available_space(&download_dir) {
            Ok(available) if available < size => {
                let needed_mb = size as f64 / 1_048_576.0;
                let avail_mb = available as f64 / 1_048_576.0;
                Err(format!(
                    "Not enough disk space. Need {:.0} MB but only {:.0} MB available in {}",
                    needed_mb,
                    avail_mb,
                    download_dir.display()
                ))
            }
            Ok(_) => Ok(()),
            Err(e) => {
                warn!("Failed to check disk space: {}", e);
                Ok(())
            }
        }
    }

    let c_clone = c.clone();
    tokio::spawn(async move {
        #[cfg(target_os = "linux")]
        let last_player_identity = Arc::new(std::sync::Mutex::new(None::<String>));

        // Track active transfers so concurrent transfers do not lose cancel
        // visibility/state when one transfer completes before the others.
        let mut outgoing_transfers: std::collections::HashMap<String, (String, f32)> =
            std::collections::HashMap::new();
        let mut incoming_transfers: std::collections::HashMap<String, (String, f32)> =
            std::collections::HashMap::new();
        // Track total file sizes per transfer for aggregate progress calculation
        let mut transfer_sizes: std::collections::HashMap<String, u64> =
            std::collections::HashMap::new();
        // Queue of outgoing transfers that failed due to connection loss, keyed by device IP.
        // When the device reconnects (DeviceFound), these are automatically retried.
        let mut pending_retry: std::collections::HashMap<
            String,
            Vec<(std::path::PathBuf, String)>,
        > = std::collections::HashMap::new();

        loop {
            let event = match events.recv().await {
                Ok(event) => event,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                    warn!(
                        "Event receiver lagged — dropped {} event(s). \
                         Consider increasing EVENT_CHANNEL_CAPACITY if this recurs.",
                        count
                    );
                    // Continue receiving; the channel is still usable after a lag.
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    info!("Event channel closed, stopping event loop");
                    break;
                }
            };
            match event {
                ConnectedEvent::DeviceFound(d) => {
                    let dev_ip = d.ip.clone();
                    let dev_port = d.port;
                    let dev_name = d.name.clone();
                    let mut info: DeviceInfo = d.clone().into();
                    info.is_trusted = c_clone.is_device_trusted(&d.id);
                    let is_trusted = info.is_trusted;

                    // If trusted, remove from pending
                    if info.is_trusted {
                        get_pending_pairings().lock_or_recover().remove(&info.id);
                    }

                    let mut store = get_devices_store().lock_or_recover();
                    store.insert(info.id.clone(), info);

                    // Auto-retry any transfers that failed due to connection loss
                    let retry_paths: Vec<std::path::PathBuf> = {
                        let by_ip = pending_retry.remove(&dev_ip).unwrap_or_default();
                        let by_fallback = pending_retry.remove("0.0.0.0").unwrap_or_default();
                        by_ip
                            .into_iter()
                            .chain(by_fallback)
                            .map(|(p, _)| p)
                            .collect()
                    };
                    if !retry_paths.is_empty() {
                        info!(
                            "Device {} reconnected — retrying {} failed transfer(s)",
                            dev_name,
                            retry_paths.len()
                        );
                        let c = c_clone.clone();
                        let ip = dev_ip.clone();
                        let port = dev_port;
                        let dname = dev_name.clone();
                        tokio::spawn(async move {
                            let mut retried = 0usize;
                            if let Ok(ip_addr) = ip.parse::<std::net::IpAddr>() {
                                for path in retry_paths {
                                    match c.send_file(ip_addr, port, path.clone()).await {
                                        Ok(transfer_id) => {
                                            store_transfer_path(transfer_id.clone(), path);
                                            set_active_outgoing_transfer_id(Some(transfer_id));
                                            retried += 1;
                                        }
                                        Err(e) => {
                                            error!(
                                                "Auto-retry failed for {}: {}",
                                                path.display(),
                                                e
                                            );
                                        }
                                    }
                                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                                }
                            }
                            if retried > 0 {
                                add_notification(
                                    "Retrying Transfer",
                                    &format!("Retrying {} file(s) to {}", retried, dname),
                                    "",
                                );
                            }
                        });
                    }

                    if d.ip != "0.0.0.0" && is_trusted {
                        save_device_to_settings(
                            d.id.clone(),
                            SavedDeviceInfo {
                                name: d.name.clone(),
                                ip: d.ip.clone(),
                                port: d.port,
                                device_type: d.device_type,
                            },
                        );
                    }
                }
                ConnectedEvent::DeviceLost(id) => {
                    let mut store = get_devices_store().lock_or_recover();
                    if let Some(device) = store.get_mut(&id) {
                        if device.is_trusted {
                            // Keep trusted devices visible as offline
                            device.ip = "0.0.0.0".to_string();
                        } else {
                            store.remove(&id);
                        }
                    }
                }
                ConnectedEvent::CompressionProgress {
                    filename,
                    current_file,
                    files_processed,
                    total_files,
                    bytes_processed,
                    total_bytes,
                    speed_bytes_per_sec,
                } => {
                    *get_transfer_status().lock_or_recover() = TransferStatus::Compressing {
                        filename,
                        current_file,
                        files_processed,
                        total_files,
                        bytes_processed,
                        total_bytes,
                        speed_bytes_per_sec,
                    };
                }
                ConnectedEvent::TransferStarting {
                    filename,
                    direction,
                    id,
                    total_size,
                    ..
                } => {
                    use connected_core::events::TransferDirection;
                    transfer_sizes.insert(id.clone(), total_size);
                    if direction == TransferDirection::Incoming {
                        incoming_transfers.insert(id.clone(), (filename.clone(), 0.0));
                        set_active_incoming_transfer_id(Some(id.clone()));
                    } else {
                        outgoing_transfers.insert(id.clone(), (filename.clone(), 0.0));
                        set_active_outgoing_transfer_id(Some(id.clone()));
                    }
                    *get_transfer_status().lock_or_recover() = TransferStatus::Starting {
                        filename: filename.clone(),
                    };
                    if direction == TransferDirection::Incoming {
                        add_notification(
                            "Transfer Starting",
                            &format!("Receiving {}", filename),
                            "",
                        );
                    }
                }
                ConnectedEvent::TransferProgress {
                    id,
                    bytes_transferred,
                    total_size,
                } => {
                    transfer_sizes.insert(id.clone(), total_size);
                    let percent = if total_size > 0 {
                        (bytes_transferred as f32 / total_size as f32) * 100.0
                    } else {
                        0.0
                    };

                    if let Some((_, last_percent)) = outgoing_transfers.get_mut(&id) {
                        *last_percent = percent;
                        set_active_outgoing_transfer_id(Some(id.clone()));
                    } else if let Some((_, last_percent)) = incoming_transfers.get_mut(&id) {
                        *last_percent = percent;
                        set_active_incoming_transfer_id(Some(id.clone()));
                    }

                    // Show multi-file aggregate when multiple transfers are active
                    let total_count = outgoing_transfers.len() + incoming_transfers.len();
                    let all_transfers: Vec<(&String, &(String, f32))> = outgoing_transfers
                        .iter()
                        .chain(incoming_transfers.iter())
                        .collect();

                    if total_count > 1 {
                        let total_bytes: u64 = all_transfers
                            .iter()
                            .filter_map(|(id, _)| transfer_sizes.get(*id))
                            .sum();
                        let total_sent: u64 = all_transfers
                            .iter()
                            .filter_map(|(id, (_, p))| {
                                transfer_sizes
                                    .get(*id)
                                    .map(|s| (*s as f64 * (*p as f64 / 100.0)) as u64)
                            })
                            .sum();
                        let aggregate_percent = if total_bytes > 0 {
                            (total_sent as f32 / total_bytes as f32) * 100.0
                        } else {
                            0.0
                        };

                        let (current_filename, _) = &all_transfers[0].1;
                        let label = format!(
                            "{}  (file {}/{})",
                            current_filename,
                            all_transfers
                                .iter()
                                .position(|(i, _)| *i == &id)
                                .unwrap_or(0)
                                + 1,
                            total_count
                        );

                        *get_transfer_status().lock_or_recover() = TransferStatus::InProgress {
                            filename: label,
                            percent: aggregate_percent,
                        };
                    } else if let Some((filename, _)) = all_transfers.first() {
                        *get_transfer_status().lock_or_recover() = TransferStatus::InProgress {
                            filename: filename.to_string(),
                            percent,
                        };
                    }
                }
                ConnectedEvent::TransferCompleted { filename, id, .. } => {
                    transfer_sizes.remove(&id);
                    let was_outgoing = outgoing_transfers.remove(&id).is_some();
                    let was_incoming = incoming_transfers.remove(&id).is_some();

                    if was_outgoing || was_incoming {
                        // Keep active transfer ids aligned with live transfer maps
                        // to avoid stale IDs causing cancel errors.
                        let next_outgoing = outgoing_transfers.keys().next().cloned();
                        let next_incoming = incoming_transfers.keys().next().cloned();
                        set_active_outgoing_transfer_id(next_outgoing);
                        set_active_incoming_transfer_id(next_incoming);
                    }

                    // If there are still transfers running, keep showing one of
                    // them instead of clearing the transfer UI.
                    if !outgoing_transfers.is_empty() || !incoming_transfers.is_empty() {
                        if let Some((next_id, (next_filename, next_percent))) = outgoing_transfers
                            .iter()
                            .next()
                            .or_else(|| incoming_transfers.iter().next())
                        {
                            if outgoing_transfers.contains_key(next_id) {
                                set_active_outgoing_transfer_id(Some(next_id.clone()));
                            } else {
                                set_active_incoming_transfer_id(Some(next_id.clone()));
                            }
                            *get_transfer_status().lock_or_recover() = TransferStatus::InProgress {
                                filename: next_filename.clone(),
                                percent: *next_percent,
                            };
                        }

                        add_notification(
                            "Transfer Complete",
                            &format!("{} finished", filename),
                            "",
                        );

                        continue;
                    }

                    set_active_outgoing_transfer_id(None);
                    set_active_incoming_transfer_id(None);
                    set_transfer_status(TransferStatus::Completed {
                        filename: filename.clone(),
                    });

                    if was_incoming {
                        let download_dir = get_download_directory_setting()
                            .map(PathBuf::from)
                            .or_else(dirs::download_dir)
                            .unwrap_or_else(|| PathBuf::from("."));
                        let received_path = download_dir.join(&filename);

                        add_open_file_notification(
                            "Transfer Complete",
                            &format!("{} received. Click to open.", filename),
                            "",
                            &received_path,
                        );
                    } else {
                        add_notification(
                            "Transfer Complete",
                            &format!("{} finished", filename),
                            "",
                        );
                    }
                }
                ConnectedEvent::TransferFailed { error, id, .. } => {
                    transfer_sizes.remove(&id);
                    remove_file_transfer_request(&id);

                    let is_cancelled = is_cancelled_error(&error);

                    let was_outgoing = outgoing_transfers.remove(&id).is_some();
                    let was_incoming = incoming_transfers.remove(&id).is_some();

                    // Queue outgoing transfers that failed due to connection loss for auto-retry
                    if was_outgoing && !is_cancelled {
                        let err_lower = error.to_lowercase();
                        let is_connection_error = err_lower.contains("timeout")
                            || err_lower.contains("eof")
                            || err_lower.contains("connection")
                            || err_lower.contains("reset")
                            || err_lower.contains("refused")
                            || err_lower.contains("unreachable")
                            || err_lower.contains("aborted");
                        if is_connection_error && let Some(path) = remove_transfer_path(&id) {
                            pending_retry
                                .entry("0.0.0.0".to_string())
                                .or_default()
                                .push((path, String::new()));
                            info!(
                                "Queued transfer {} for auto-retry on reconnect (error: {})",
                                id, error
                            );
                        }
                    }

                    if was_outgoing || was_incoming {
                        // Keep active transfer ids aligned with live transfer maps
                        // to avoid stale IDs causing cancel errors.
                        let next_outgoing = outgoing_transfers.keys().next().cloned();
                        let next_incoming = incoming_transfers.keys().next().cloned();
                        set_active_outgoing_transfer_id(next_outgoing);
                        set_active_incoming_transfer_id(next_incoming);
                    }

                    // If another transfer is still active, keep it visible and cancellable.
                    if !outgoing_transfers.is_empty() || !incoming_transfers.is_empty() {
                        if let Some((next_id, (next_filename, next_percent))) = outgoing_transfers
                            .iter()
                            .next()
                            .or_else(|| incoming_transfers.iter().next())
                        {
                            if outgoing_transfers.contains_key(next_id) {
                                set_active_outgoing_transfer_id(Some(next_id.clone()));
                            } else {
                                set_active_incoming_transfer_id(Some(next_id.clone()));
                            }
                            *get_transfer_status().lock_or_recover() = TransferStatus::InProgress {
                                filename: next_filename.clone(),
                                percent: *next_percent,
                            };
                        }

                        if !is_cancelled {
                            add_notification("Transfer Failed", &error, "");
                        }

                        continue;
                    }

                    set_active_outgoing_transfer_id(None);
                    set_active_incoming_transfer_id(None);
                    if is_cancelled {
                        set_transfer_status(TransferStatus::Cancelled {
                            filename: String::from("File transfer"),
                        });
                    } else {
                        set_transfer_status(TransferStatus::Failed {
                            error: error.clone(),
                        });
                        add_notification("Transfer Failed", &error, "");
                    }
                }
                ConnectedEvent::ClipboardReceived {
                    content,
                    from_device,
                } => {
                    set_system_clipboard(&content);
                    *get_last_clipboard().lock_or_recover() = content.clone();
                    // Store the remote content to prevent echo loops
                    set_last_remote_clipboard_content(content.clone());
                    *get_last_remote_update().lock_or_recover() = Instant::now();
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
                    if !*get_pairing_mode_state().lock_or_recover()
                        && !c_clone.is_device_trusted(&device_id)
                    {
                        info!(
                            "Auto-rejecting pairing request while pairing mode is disabled: {} ({})",
                            device_name, device_id
                        );
                        let c_reject = c_clone.clone();
                        let did = device_id.clone();
                        tokio::spawn(async move {
                            if let Err(e) = c_reject.reject_pairing(&did).await {
                                warn!("Failed to auto-reject pairing request: {}", e);
                            }
                        });
                        continue;
                    }

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
                        continue;
                    }

                    // Deduplicate requests
                    let mut requests = get_pairing_requests().lock_or_recover();
                    if !requests.iter().any(|r| r.fingerprint == fingerprint) {
                        add_actionable_notification(
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
                    reason,
                } => {
                    add_notification(
                        "Pairing Update",
                        &format!("Pairing {} by {}", reason, device_name),
                        "",
                    );
                    get_pending_pairings().lock_or_recover().remove(&device_id);
                    get_pairing_requests()
                        .lock_or_recover()
                        .retain(|r| r.device_id != device_id);
                }
                ConnectedEvent::PairingModeChanged(enabled) => {
                    info!("Pairing mode changed: {}", enabled);
                    set_pairing_mode_state(enabled);
                }
                ConnectedEvent::DeviceUnpaired {
                    device_id,
                    device_name,
                } => {
                    // Update local store - device is no longer trusted
                    {
                        let mut store = get_devices_store().lock_or_recover();
                        if let Some(d) = store.get_mut(&device_id) {
                            d.is_trusted = false;
                        }
                    }

                    remove_device_from_settings(&device_id);

                    // If device is not discovered (offline), remove it from UI
                    let is_discovered = c_clone
                        .get_discovered_devices()
                        .iter()
                        .any(|d| d.id == device_id);
                    if !is_discovered {
                        let mut store = get_devices_store().lock_or_recover();
                        store.remove(&device_id);
                    }

                    add_notification(
                        "Device Disconnected",
                        &format!("You were unpaired from {}", device_name),
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
                    // Check disk space before accepting
                    if let Err(msg) = check_disk_space(size) {
                        warn!(
                            "Rejecting transfer {} from {}: {}",
                            filename, from_device, msg
                        );
                        let _ = c_clone.reject_file_transfer(&id);
                        add_notification("Transfer Rejected", &msg, "");
                        *get_transfer_status().lock_or_recover() = TransferStatus::Idle;
                        continue;
                    }

                    if is_auto_accept_enabled(&from_fingerprint) {
                        info!("Auto-accepting transfer {} from {}", filename, from_device);
                        if let Err(e) = c_clone.accept_file_transfer(&id) {
                            error!("Failed to auto-accept transfer: {}", e);
                        }
                    } else {
                        *get_transfer_status().lock_or_recover() = TransferStatus::Pending {
                            transfer_id: id.clone(),
                            filename: filename.clone(),
                            from_device: from_device.clone(),
                        };

                        add_actionable_notification(
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
                }
                ConnectedEvent::MediaControl { from_device, event } => {
                    info!("EVENT: Received MediaControl event from {}", from_device);
                    // Only process if media control is enabled locally
                    if *get_media_enabled().lock_or_recover() {
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

                                        // Volume commands are system-level and don't need
                                        // a media player.  Handle them via pactl before
                                        // the MPRIS lookup so they work even when no
                                        // player is running (matching the Windows path).
                                        match cmd {
                                            MediaCommand::VolumeUp => {
                                                let _ = std::process::Command::new("pactl")
                                                    .args([
                                                        "set-sink-volume",
                                                        "@DEFAULT_SINK@",
                                                        "+5%",
                                                    ])
                                                    .spawn();
                                                return;
                                            }
                                            MediaCommand::VolumeDown => {
                                                let _ = std::process::Command::new("pactl")
                                                    .args([
                                                        "set-sink-volume",
                                                        "@DEFAULT_SINK@",
                                                        "-5%",
                                                    ])
                                                    .spawn();
                                                return;
                                            }
                                            MediaCommand::Mute => {
                                                let _ = std::process::Command::new("pactl")
                                                    .args([
                                                        "set-sink-mute",
                                                        "@DEFAULT_SINK@",
                                                        "toggle",
                                                    ])
                                                    .spawn();
                                                return;
                                            }
                                            _ => {} // Playback commands need MPRIS below
                                        }

                                        // Use cached MPRIS names to avoid repeated DBus queries
                                        let mpris_names = match get_cached_mpris_names() {
                                            Some(names) => names,
                                            None => {
                                                warn!("Could not get MPRIS player names");
                                                return;
                                            }
                                        };

                                        let last_id = last_identity.lock_or_recover().clone();

                                        let mut playing_player: Option<Player> = None;

                                        let mut preferred_player: Option<Player> = None;

                                        let mut generic_paused: Option<Player> = None;

                                        let mut generic_any: Option<Player> = None;

                                        for name in &mpris_names {
                                            if let Ok(p_conn) =
                                                Connection::get_private(BusType::Session)
                                                && let Ok(player) =
                                                    Player::new(p_conn, name.clone(), 1500)
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

                                            *last_identity.lock_or_recover() =
                                                Some(player.identity().to_string());

                                            info!(
                                                "MPRIS: Controlling player: {}",
                                                player.identity()
                                            );

                                            let res = match cmd {
                                                MediaCommand::Play => {
                                                    player.checked_play().map(|_| ())
                                                }
                                                MediaCommand::Pause => {
                                                    player.checked_pause().map(|_| ())
                                                }
                                                MediaCommand::PlayPause => {
                                                    player.checked_play_pause().map(|_| ())
                                                }
                                                MediaCommand::Next => {
                                                    player.checked_next().map(|_| ())
                                                }
                                                MediaCommand::Previous => {
                                                    player.checked_previous().map(|_| ())
                                                }
                                                MediaCommand::Stop => {
                                                    player.checked_stop().map(|_| ())
                                                }
                                                // Volume commands are handled above via
                                                // pactl before the MPRIS lookup.
                                                MediaCommand::VolumeUp
                                                | MediaCommand::VolumeDown
                                                | MediaCommand::Mute => Ok(()),
                                            };

                                            if let Err(e) = res {
                                                warn!("MPRIS Command Error: {}", e);
                                            }
                                        } else {
                                            warn!("No controllable media player found");
                                        }
                                    }
                                    #[cfg(target_os = "macos")]
                                    {
                                        if let Err(e) = crate::macos_media::control_media(cmd) {
                                            warn!("macOS Media Control Error: {}", e);
                                            add_notification("Media Control", &e, "");
                                        }
                                    }
                                    #[cfg(target_os = "windows")]
                                    {
                                        use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager;

                                        async fn control_media_windows(
                                            cmd: MediaCommand,
                                        ) -> windows::core::Result<()>
                                        {
                                            match cmd {
                                                MediaCommand::VolumeUp
                                                | MediaCommand::VolumeDown => {
                                                    // Use system volume control since SMTC doesn't have volume
                                                    return crate::windows_audio::control_system_volume(cmd);
                                                }
                                                _ => {
                                                    // Use SMTC for playback control
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
                                                            session
                                                                .TryTogglePlayPauseAsync()?
                                                                .await?;
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
                                                        MediaCommand::VolumeUp
                                                        | MediaCommand::VolumeDown
                                                        | MediaCommand::Mute => {}
                                                    }
                                                }
                                            }
                                            Ok(())
                                        }

                                        if let Err(e) =
                                            runtime_handle.block_on(control_media_windows(cmd))
                                        {
                                            warn!("Windows Media Control Error: {}", e);
                                            let error_msg = match e.code().0 {
                                                // ERROR_INVALID_STATE - no media session available
                                                -2147024809 => {
                                                    "No media is currently playing".to_string()
                                                }
                                                // ERROR_NOT_FOUND - no active media session
                                                -2147023728 => "No media player found".to_string(),
                                                // E_ACCESSDENIED - no permission for audio
                                                -2147024891 => {
                                                    "No permission to control audio".to_string()
                                                }
                                                // E_FAIL - generic failure
                                                _ => format!("Media control failed ({})", e),
                                            };
                                            add_notification("Media Control", &error_msg, "");
                                        }
                                    }
                                    #[cfg(not(any(
                                        target_os = "linux",
                                        target_os = "macos",
                                        target_os = "windows"
                                    )))]
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
                                    let store = get_devices_store().lock_or_recover();
                                    store
                                        .values()
                                        .find(|d| d.name == from_device)
                                        .map(|d| d.id.clone())
                                        .unwrap_or_else(|| "unknown".to_string())
                                };

                                let state_for_mpris = state.clone();
                                *get_last_remote_media_device_id().lock_or_recover() =
                                    Some(device_id.clone());
                                *get_current_media().lock_or_recover() = Some(RemoteMedia {
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
                                let mut convos = get_phone_conversations().lock_or_recover();
                                if let Some(convo) = convos.iter_mut().find(|c| c.id == thread_id) {
                                    convo.last_message = Some(latest.body.clone());
                                    convo.last_timestamp = latest.timestamp;
                                }
                                convos.sort_by_key(|b| std::cmp::Reverse(b.last_timestamp));
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
                                let mut msgs = get_phone_messages().lock_or_recover();
                                if let Some(thread_msgs) = msgs.get_mut(&thread_id) {
                                    thread_msgs.push(message.clone());
                                } else {
                                    msgs.insert(thread_id.clone(), vec![message.clone()]);
                                }
                            }

                            // Update conversation list with new message
                            {
                                let mut convos = get_phone_conversations().lock_or_recover();

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
                                convos.sort_by_key(|b| std::cmp::Reverse(b.last_timestamp));
                            }

                            *get_phone_data_update().lock_or_recover() = std::time::Instant::now();
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
                                let now_ms = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_millis();

                                // If call ended, add to call log
                                if c.state == ActiveCallState::Ended {
                                    let entry = CallLogEntry {
                                        id: format!("call_{}", now_ms),
                                        number: c.number.clone(),
                                        contact_name: c.contact_name.clone(),
                                        call_type: if c.is_incoming {
                                            CallType::Incoming
                                        } else {
                                            CallType::Outgoing
                                        },
                                        timestamp: now_ms as u64,
                                        duration: c.duration,
                                        is_read: false,
                                    };
                                    // Add to beginning of call log
                                    let mut log = get_phone_call_log().lock_or_recover();
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

            // Mark SDK as initialized and discovery as active
            set_sdk_initialized(true);
            set_discovery_active(true);

            // Register Filesystem Provider
            c.register_filesystem_provider(Box::new(DesktopFilesystemProvider::new()));

            // Spawn event loop
            spawn_event_loop(c.clone(), c.subscribe());

            // Re-fetch any devices discovered during startup.
            // Between start_listening/browse and spawn_event_loop subscribing,
            // DeviceFound events hit a broadcast channel with no subscribers and
            // are silently dropped. Repopulate the store from what the discovery
            // layer already knows about (matches Android FFI's start_discovery pattern).
            for d in c.get_discovered_devices() {
                let mut info: DeviceInfo = d.into();
                info.is_trusted = c.is_device_trusted(&info.id);
                if info.is_trusted {
                    get_pending_pairings().lock_or_recover().remove(&info.id);
                }
                get_devices_store()
                    .lock_or_recover()
                    .insert(info.id.clone(), info);
            }

            // Add trusted devices that are currently offline (not discovered via mDNS).
            // These are shown in a separate "Offline" section so users can manage them
            // (unpair) without waiting for the device to reconnect.
            {
                let saved = get_saved_devices_setting();
                let store = get_devices_store().lock_or_recover();
                let missing: Vec<_> = saved
                    .iter()
                    .filter(|(device_id, info)| {
                        !store.contains_key(*device_id) && info.ip != "0.0.0.0"
                    })
                    .filter(|(device_id, _)| c.is_device_trusted(device_id))
                    .map(|(device_id, info)| {
                        (
                            device_id.clone(),
                            DeviceInfo {
                                id: device_id.clone(),
                                name: info.name.clone(),
                                ip: "0.0.0.0".to_string(),
                                port: info.port,
                                device_type: info.device_type,
                                is_trusted: true,
                                is_pending: false,
                            },
                        )
                    })
                    .collect();
                drop(store);
                let mut store = get_devices_store().lock_or_recover();
                for (device_id, device) in missing {
                    store.insert(device_id, device);
                }
            }

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
    let mut clipboard_monitor: Option<tokio::task::JoinHandle<()>> = None;

    while let Some(action) = rx.recv().await {
        match action {
            AppAction::Init => {
                if client.is_some() {
                    continue;
                }
                let name = get_device_name_setting().unwrap_or_else(get_hostname);
                client = start_core(name).await;
                if let Some(c) = &client {
                    if let Some(handle) = clipboard_monitor.take() {
                        handle.abort();
                    }
                    clipboard_monitor = Some(spawn_clipboard_monitor(c.clone()));
                    c.set_pairing_mode_persistent(true);
                }
                // Apply saved download directory to core
                if let Some(c) = &client
                    && let Some(dir) = get_download_directory_setting()
                {
                    if let Err(e) = c.set_download_dir(PathBuf::from(&dir)) {
                        error!("Failed to apply saved download directory: {}", e);
                    } else {
                        info!("Applied saved download directory: {}", dir);
                    }
                }

                let actual_autostart = crate::autostart::is_enabled();
                let saved_autostart = get_autostart_enabled_setting();
                if actual_autostart
                    && saved_autostart
                    && let Err(e) = crate::autostart::set_enabled(true)
                {
                    error!("Failed to refresh autostart entry: {}", e);
                }
                if actual_autostart != saved_autostart {
                    set_autostart_enabled_setting(actual_autostart);
                }
            }
            AppAction::RenameDevice { new_name } => {
                set_device_name_setting(new_name.clone());
                if let Some(c) = &client
                    && let Err(e) = c.rename_local_device(new_name)
                {
                    error!("Failed to rename local device: {}", e);
                }
            }
            AppAction::SendFile { ip, port, paths } => {
                if let Some(c) = &client {
                    if ip == "0.0.0.0" {
                        add_notification(
                            "Offline Mode",
                            "Offline transfer not supported. Connect to same network.",
                            "",
                        );
                        continue;
                    }
                    let c = c.clone();
                    let ip_clone = ip.clone();
                    tokio::spawn(async move {
                        if let Ok(ip_addr) = ip_clone.parse() {
                            for path in paths {
                                let path_buf = PathBuf::from(&path);
                                match c.send_file(ip_addr, port, path_buf.clone()).await {
                                    Ok(transfer_id) => {
                                        store_transfer_path(transfer_id.clone(), path_buf);
                                        set_active_outgoing_transfer_id(Some(transfer_id));
                                    }
                                    Err(e) => {
                                        error!("Failed to start file transfer: {}", e);
                                    }
                                }
                                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                            }
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
                        continue;
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
                        continue;
                    }

                    // Find device ID by IP/Port to add to pending
                    let device_id = {
                        let store = get_devices_store().lock_or_recover();
                        store
                            .values()
                            .find(|d| d.ip == ip && d.port == port)
                            .map(|d| d.id.clone())
                    };

                    if let Some(did) = device_id.clone() {
                        get_pending_pairings().lock_or_recover().insert(did);
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
                                        get_pending_pairings().lock_or_recover().remove(&did);
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to send handshake: {}", e);
                                    // Suppress notification if rejected/cancelled, as events or the local UI handle it.
                                    let error = e.to_string();
                                    let lower = error.to_lowercase();
                                    if !lower.contains("rejected")
                                        && !lower.contains("cancelled")
                                        && !lower.contains("canceled")
                                    {
                                        add_notification("Pairing Failed", &e.to_string(), "");
                                    }
                                    // Remove from pending on failure
                                    if let Some(did) = device_id_for_cleanup {
                                        get_pending_pairings().lock_or_recover().remove(&did);
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
                                let mut requests = get_pairing_requests().lock_or_recover();
                                requests.retain(|r| r.fingerprint != fingerprint);
                            }

                            // Update device trust status in store
                            {
                                let mut store = get_devices_store().lock_or_recover();
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
            AppAction::CancelPairing { device_id } => {
                get_pending_pairings().lock_or_recover().remove(&device_id);

                if let Some(c) = &client {
                    let c = c.clone();
                    tokio::spawn(async move {
                        if let Err(e) = c.cancel_pairing(&device_id).await {
                            warn!("Failed to cancel pairing with {}: {}", device_id, e);
                        }
                    });
                }
            }
            AppAction::RejectDevice {
                fingerprint,
                device_id,
            } => {
                if let Some(c) = &client {
                    // Remove from pairing requests
                    {
                        let mut requests = get_pairing_requests().lock_or_recover();
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
                    c.set_pairing_mode_persistent(enabled);
                }
            }
            AppAction::AcceptFileTransfer { transfer_id } => {
                if let Some(c) = &client {
                    // Remove from pending requests
                    remove_file_transfer_request(&transfer_id);

                    let mut status = get_transfer_status().lock_or_recover();
                    if let TransferStatus::Pending {
                        transfer_id: pending_id,
                        ..
                    } = &*status
                        && pending_id == &transfer_id
                    {
                        *status = TransferStatus::Starting {
                            filename: String::from("Preparing transfer..."),
                        };
                    }
                    drop(status);

                    // Set active incoming transfer ID immediately so the
                    // cancel button works even before TransferStarting arrives.
                    set_active_incoming_transfer_id(Some(transfer_id.clone()));

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

                    let mut status = get_transfer_status().lock_or_recover();
                    if let TransferStatus::Pending {
                        transfer_id: pending_id,
                        ..
                    } = &*status
                        && pending_id == &transfer_id
                    {
                        *status = TransferStatus::Idle;
                    }

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
            AppAction::CancelFileTransfer => {
                if let Some(c) = &client {
                    let outgoing_id = get_active_outgoing_transfer_id().lock_or_recover().clone();
                    if let Some(id) = outgoing_id {
                        let _ = c.cancel_file_transfer(&id);
                        set_active_outgoing_transfer_id(None);
                        remove_file_transfer_request(&id);
                        set_transfer_status(TransferStatus::Cancelled {
                            filename: String::from("File transfer"),
                        });
                        add_notification(
                            "Transfer Cancelled",
                            "Outgoing file transfer has been cancelled",
                            "",
                        );
                        continue;
                    }

                    let incoming_id = get_active_incoming_transfer_id().lock_or_recover().clone();
                    if let Some(id) = incoming_id {
                        let _ = c.cancel_incoming_file_transfer(&id);
                        set_active_incoming_transfer_id(None);
                        remove_file_transfer_request(&id);
                        set_transfer_status(TransferStatus::Cancelled {
                            filename: String::from("File transfer"),
                        });
                        add_notification(
                            "Transfer Cancelled",
                            "Incoming file transfer has been cancelled",
                            "",
                        );
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
                                    *get_current_remote_files().lock_or_recover() = Some(entries);
                                    *get_current_remote_path().lock_or_recover() = path;
                                    *get_remote_files_update().lock_or_recover() = Instant::now();
                                }
                                Err(e) => {
                                    error!("Failed to list remote files: {}", e);
                                    add_notification(
                                        "File Browser",
                                        &format!("Failed to list: {}", e),
                                        "",
                                    );
                                    // Trigger UI update to stop loading spinner even on failure.
                                    *get_remote_files_update().lock_or_recover() = Instant::now();
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
                            let download_dir = get_download_directory_setting()
                                .map(PathBuf::from)
                                .unwrap_or_else(|| {
                                    dirs::download_dir().unwrap_or_else(|| PathBuf::from("."))
                                });
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
                            let ext = std::path::Path::new(&filename)
                                .extension()
                                .and_then(|e| e.to_str())
                                .filter(|e| !e.is_empty())
                                .map(|e| e.to_string())
                                .unwrap_or_else(|| "bin".to_string());
                            let local_path = temp_dir.join(format!(
                                "connected-preview-{}.{}",
                                uuid::Uuid::new_v4(),
                                ext
                            ));

                            add_notification("Preview", &format!("Loading {}...", filename), "");

                            let mime = mime_guess::from_path(&filename)
                                .first_or_octet_stream()
                                .to_string();
                            let is_media = mime.starts_with("audio/") || mime.starts_with("video/");

                            match c
                                .fs_download_file(ip_addr, port, remote_path, local_path.clone())
                                .await
                            {
                                Ok(_) => {
                                    if is_media {
                                        // The desktop webview cannot play audio/video from
                                        // custom schemes, so hand the file off to a real
                                        // media player. Fall back to saving in Downloads.
                                        if !open_in_media_player(&local_path) {
                                            warn!("No media player found; saving to Downloads");
                                            if let Some(dir) = get_download_directory_setting() {
                                                let dest = std::path::Path::new(&dir).join(&filename);
                                                let _ = std::fs::copy(&local_path, &dest);
                                                add_notification(
                                                    "Preview",
                                                    &format!(
                                                        "No media player found. Saved to {}",
                                                        dest.display()
                                                    ),
                                                    "",
                                                );
                                            } else {
                                                add_notification(
                                                    "Preview",
                                                    "No media player found to open this file",
                                                    "",
                                                );
                                            }
                                            let _ = tokio::fs::remove_file(&local_path).await;
                                            // Surface the failure in the app (toasts may be
                                            // hidden) so the user at least sees a response.
                                            *get_preview_data().lock_or_recover() =
                                                Some(PreviewData {
                                                    filename,
                                                    mime_type: mime,
                                                    data: Vec::new(),
                                                    local_path: None,
                                                });
                                        }
                                    } else {
                                        let read_res = tokio::fs::read(&local_path).await;
                                        let _ = tokio::fs::remove_file(&local_path).await;

                                        if let Ok(data) = read_res {
                                            *get_preview_data().lock_or_recover() =
                                                Some(PreviewData {
                                                    filename,
                                                    mime_type: mime,
                                                    data,
                                                    local_path: None,
                                                });
                                        } else {
                                            add_notification(
                                                "Preview",
                                                "Failed to read file",
                                                "",
                                            );
                                        }
                                    }
                                }
                                Err(e) => {
                                    let _ = tokio::fs::remove_file(&local_path).await;
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
                                        get_thumbnails().lock_or_recover().insert(path, data);
                                        *get_thumbnails_update().lock_or_recover() =
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
                if let Some(name) = get_preview_data()
                    .lock_or_recover()
                    .as_ref()
                    .and_then(|d| d.local_path.clone())
                {
                    let _ = std::fs::remove_file(std::env::temp_dir().join(name));
                }
                *get_preview_data().lock_or_recover() = None;
            }
            AppAction::ToggleMediaControl { enabled, notify } => {
                *get_media_enabled().lock_or_recover() = enabled;
                if enabled {
                    info!("Media Poller Started");
                    if let Some(c) = &client {
                        let c = c.clone();
                        // Start MPRIS poller with longer interval to reduce CPU usage
                        tokio::spawn(async move {
                            // Use 2 second interval instead of 1 second to reduce CPU wakeups
                            let mut interval =
                                tokio::time::interval(std::time::Duration::from_secs(2));
                            let mut last_title = String::new();
                            let mut last_playing = false;
                            let mut consecutive_no_change = 0u32;
                            const MAX_CONSECUTIVE_NO_CHANGE: u32 = 15; // Cap to prevent infinite growth

                            // We need to check the atomic flag in the loop
                            while *get_media_enabled().lock_or_recover() {
                                interval.tick().await;

                                // Adaptive polling: if nothing changes for a while, slow down
                                // Cap the counter to prevent repeated long sleeps from accumulating
                                if consecutive_no_change > 10
                                    && consecutive_no_change <= MAX_CONSECUTIVE_NO_CHANGE
                                {
                                    // After 20 seconds of no change, add extra delay (poll every ~5 seconds)
                                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                                }

                                #[cfg(target_os = "windows")]
                                let runtime_handle = tokio::runtime::Handle::current();

                                let state_update: Option<MediaPollStateUpdate> =
                                    tokio::task::spawn_blocking(
                                        move || -> Option<MediaPollStateUpdate> {
                                            #[cfg(target_os = "linux")]
                                            {
                                                // Manual D-Bus scan to bypass broken playerctld
                                                use dbus::ffidisp::{BusType, Connection};
                                                use mpris::Player;

                                                // Use cached MPRIS names to avoid repeated DBus queries
                                                let mpris_names = match get_cached_mpris_names() {
                                                    Some(names) => names,
                                                    None => return None,
                                                };

                                                // Find first playing, or just first one
                                                let mut best_candidate: Option<MediaPollStateUpdate> = None;

                                                for name in &mpris_names {
                                                    // Player::new takes (conn, bus_name, timeout_ms)
                                                    // We must create a new connection for each player because Player::new takes ownership
                                                    if let Ok(p_conn) =
                                                        Connection::get_private(BusType::Session)
                                                        && let Ok(player) =
                                                            Player::new(p_conn, name.clone(), 1500)
                                                    {
                                                        let _identity = player.identity().to_string();
                                                        let meta = player.get_metadata().ok();
                                                        let status = player.get_playback_status().ok();
                                                        let playing = matches!(
                                                            status,
                                                            Some(PlaybackStatus::Playing)
                                                        );

                                                        let title = meta.as_ref().and_then(|m| {
                                                            m.title().map(|s| s.to_string())
                                                        });
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

                                                runtime_handle
                                                    .block_on(get_media_state_windows())
                                                    .unwrap_or_default()
                                            }

                                            #[cfg(target_os = "macos")]
                                            {
                                                crate::macos_media::current_media_state().map(
                                                    |state| {
                                                        (
                                                            state.title,
                                                            state.artist,
                                                            state.album,
                                                            state.playing,
                                                        )
                                                    },
                                                )
                                            }

                                            #[cfg(not(any(
                                                target_os = "linux",
                                                target_os = "macos",
                                                target_os = "windows"
                                            )))]
                                            {
                                                None
                                            }
                                        },
                                    )
                                .await
                                .unwrap_or(None);

                                if let Some((title, artist, album, playing)) = state_update {
                                    let current_title: String = title.clone().unwrap_or_default();

                                    // Only send update if changed
                                    if current_title != last_title || playing != last_playing {
                                        last_title = current_title;
                                        last_playing = playing;
                                        consecutive_no_change = 0; // Reset adaptive polling counter on change

                                        let state = MediaState {
                                            title,
                                            artist,
                                            album,
                                            playing,
                                        };

                                        // Update local state too
                                        *get_current_media().lock_or_recover() =
                                            Some(RemoteMedia {
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
                                } else {
                                    // Cap to prevent unbounded growth
                                    if consecutive_no_change < MAX_CONSECUTIVE_NO_CHANGE {
                                        consecutive_no_change += 1;
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
                            .lock_or_recover()
                            .clone()
                            .unwrap_or_default();
                        if device_id.is_empty() || device_id == "local" || device_id == "unknown" {
                            None
                        } else {
                            let store = get_devices_store().lock_or_recover();
                            store.get(&device_id).cloned()
                        }
                    };

                    if let Some(device) = device {
                        if device.ip == "0.0.0.0" {
                            warn!("Remote media control unavailable in offline mode");
                            continue;
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
                info!(
                    "Refreshed discovery due to adapter state change - performing lightweight refresh"
                );
                if let Some(c) = &client {
                    c.refresh_discovery();

                    let saved = get_saved_devices_setting();
                    for (device_id, info) in saved {
                        if info.ip == "0.0.0.0" {
                            continue;
                        }
                        let device_type = info.device_type;
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
            AppAction::RefreshDevices => {
                if let Some(c) = &client {
                    info!("Lightweight discovery refresh requested by user");
                    // Clear UI device store so user sees the refresh
                    get_devices_store().lock_or_recover().clear();
                    c.refresh_discovery();
                    // Re-fetch after a short delay to let mDNS browse loop pick up responses
                    let c_refresh = c.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        let discovered = c_refresh.get_discovered_devices();
                        let mut store = get_devices_store().lock_or_recover();
                        for d in discovered {
                            let mut info: DeviceInfo = d.clone().into();
                            info.is_trusted = c_refresh.is_device_trusted(&d.id);
                            store.insert(info.id.clone(), info);
                        }
                    });
                }
            }
            AppAction::SetDownloadDirectory { path } => {
                set_download_directory_setting(Some(path.clone()));
                if let Some(c) = &client {
                    if let Err(e) = c.set_download_dir(PathBuf::from(&path)) {
                        error!("Failed to set download directory: {}", e);
                        add_notification(
                            "Error",
                            &format!("Failed to set download directory: {}", e),
                            "",
                        );
                    } else {
                        info!("Download directory set to: {}", path);
                        add_notification(
                            "Settings",
                            &format!("Download directory set to {}", path),
                            "",
                        );
                    }
                }
            }
            AppAction::SetSharedFolder { path } => {
                set_shared_folder_setting(Some(path.clone()));
                if let Some(c) = &client {
                    c.register_filesystem_provider(Box::new(DesktopFilesystemProvider::with_root(
                        PathBuf::from(&path),
                    )));
                    info!("Shared folder set to: {}", path);
                    add_notification("Settings", &format!("Shared folder set to {}", path), "");
                }
            }
            AppAction::SetAutostart(enabled) => match crate::autostart::set_enabled(enabled) {
                Ok(()) => {
                    let actual_state = crate::autostart::is_enabled();
                    set_autostart_enabled_setting(actual_state);

                    if actual_state == enabled {
                        info!("Autostart set to {}", enabled);
                        add_notification(
                            "Settings",
                            if enabled {
                                "Connected will start automatically when you log in"
                            } else {
                                "Connected will no longer start automatically"
                            },
                            "",
                        );
                    } else {
                        warn!(
                            "Autostart request did not match final state (requested {}, actual {})",
                            enabled, actual_state
                        );
                        add_notification(
                            "Settings",
                            "Autostart state could not be verified. Check your system startup settings.",
                            "",
                        );
                    }
                }
                Err(e) => {
                    error!("Failed to update autostart setting: {}", e);
                    set_autostart_enabled_setting(crate::autostart::is_enabled());
                    add_notification(
                        "Settings",
                        &format!("Failed to update startup setting: {e}"),
                        "",
                    );
                }
            },
            AppAction::CheckForUpdates => {
                #[cfg(target_os = "windows")]
                {
                    add_notification(
                        "Updates",
                        "Windows updates are managed by Microsoft Store.",
                        "",
                    );
                }

                #[cfg(not(target_os = "windows"))]
                {
                    tokio::spawn(async move {
                        let platform = if cfg!(target_os = "linux") {
                            "linux"
                        } else if cfg!(target_os = "macos") {
                            "macos"
                        } else if cfg!(target_os = "windows") {
                            "windows"
                        } else {
                            "unknown"
                        };

                        let current_version = env!("CARGO_PKG_VERSION").to_string();

                        match UpdateChecker::check_for_updates(
                            current_version,
                            platform.to_string(),
                        )
                        .await
                        {
                            Ok(info) => {
                                *crate::state::get_update_info().lock_or_recover() =
                                    Some(info.clone());
                                if info.has_update {
                                    add_notification(
                                        "Update Available",
                                        &format!("Version {} is available", info.latest_version),
                                        "",
                                    );
                                } else {
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
            }
            AppAction::PerformUpdate => {
                #[cfg(target_os = "windows")]
                {
                    add_notification(
                        "Updates",
                        "Windows updates are managed by Microsoft Store.",
                        "",
                    );
                }

                #[cfg(not(target_os = "windows"))]
                {
                    let info_opt = crate::state::get_update_info().lock_or_recover().clone();
                    if let Some(info) = info_opt {
                        if let Some(url) = info.download_url {
                            #[cfg(target_os = "macos")]
                            {
                                add_notification("Updates", "Downloading update…", "");
                                let url_clone = url.clone();
                                tokio::spawn(async move {
                                    match install_macos_update(&url_clone).await {
                                        Ok(()) => {
                                            info!("macOS update installed, restarting…");
                                        }
                                        Err(e) => {
                                            error!("macOS update failed: {}", e);
                                            add_notification("Update Failed", &e.to_string(), "");
                                        }
                                    }
                                });
                            }

                            #[cfg(target_os = "linux")]
                            {
                                if is_running_as_appimage() || is_installed_via_flatpak() {
                                    let label = if is_running_as_appimage() {
                                        "AppImage"
                                    } else {
                                        "Flatpak"
                                    };
                                    add_notification("Updates", "Downloading update…", "");
                                    let url_clone = url.clone();
                                    tokio::spawn(async move {
                                        let result = if is_running_as_appimage() {
                                            install_linux_appimage_update(&url_clone).await
                                        } else {
                                            install_linux_flatpak_update(&url_clone).await
                                        };
                                        match result {
                                            Ok(()) => {
                                                info!("{} update installed, restarting…", label);
                                            }
                                            Err(e) => {
                                                error!("{} update failed: {}", label, e);
                                                add_notification(
                                                    "Update Failed",
                                                    &e.to_string(),
                                                    "",
                                                );
                                            }
                                        }
                                    });
                                } else {
                                    info!("Opening update URL: {}", url);
                                    if let Err(e) =
                                        std::process::Command::new("xdg-open").arg(&url).spawn()
                                    {
                                        error!("Failed to open update URL: {}", e);
                                        add_notification("Update", "Failed to open update URL", "");
                                    }
                                }
                            }

                            #[cfg(not(any(
                                target_os = "linux",
                                target_os = "macos",
                                target_os = "windows"
                            )))]
                            {
                                info!("Opening update URL: {}", url);
                            }
                        } else {
                            add_notification("Update", "No download URL found", "");
                        }
                    }
                }
            }
            #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
            AppAction::PickAndSendFiles => {
                let device = crate::state::get_default_device();
                if let Some(device) = device {
                    let c = client.clone();
                    tokio::spawn(async move {
                        if let Some(handles) = rfd::AsyncFileDialog::new()
                            .set_title("Select files to send")
                            .pick_files()
                            .await
                        {
                            let paths: Vec<String> = handles
                                .into_iter()
                                .map(|h| h.path().display().to_string())
                                .collect();

                            if !paths.is_empty()
                                && let (Some(c), Ok(ip)) = (c, device.ip.parse())
                            {
                                for path in paths {
                                    let _ = c.send_file(ip, device.port, PathBuf::from(path)).await;
                                }
                            }
                        }
                    });
                }
            }
            AppAction::Shutdown => {
                info!("Shutdown requested — cancelling active transfers");
                if let Some(c) = &client {
                    c.cancel_all_file_transfers();
                }
                set_active_outgoing_transfer_id(None);
                set_active_incoming_transfer_id(None);
                set_transfer_status(TransferStatus::Idle);
                break;
            }
        }
    }

    if let Some(handle) = clipboard_monitor {
        handle.abort();
    }
}
