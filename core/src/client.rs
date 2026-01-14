use crate::device::{Device, DeviceType};
use crate::discovery::{DiscoveryEvent, DiscoveryService, DiscoverySource};
use crate::error::{ConnectedError, Result};
use crate::events::{ConnectedEvent, TransferDirection};
use crate::file_transfer::{FileTransfer, TransferProgress};
use crate::security::KeyStore;
use crate::transport::{Message, QuicTransport, UnpairReason};
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::{debug, error, info, warn};

type PendingTransferSender = oneshot::Sender<bool>;

const EVENT_CHANNEL_CAPACITY: usize = 100;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
const PAIRING_MODE_TIMEOUT: Duration = Duration::from_secs(120);

pub struct ConnectedClient {
    local_device: Device,
    discovery: Arc<DiscoveryService>,
    transport: Arc<QuicTransport>,
    event_tx: broadcast::Sender<ConnectedEvent>,
    key_store: Arc<RwLock<KeyStore>>,
    download_dir: PathBuf,
    auto_accept_files: Arc<AtomicBool>,
    pairing_mode_handle: Arc<RwLock<Option<tokio::task::JoinHandle<()>>>>,
    pending_transfers: Arc<RwLock<HashMap<String, PendingTransferSender>>>,
    /// Track IPs we've sent handshakes to, so we can auto-trust their acks even if pairing mode times out
    pending_handshakes: Arc<RwLock<HashSet<IpAddr>>>,
    fs_provider: Arc<RwLock<Option<Box<dyn crate::filesystem::FilesystemProvider>>>>,
}

impl ConnectedClient {
    pub async fn new(
        device_name: String,
        device_type: DeviceType,
        port: u16,
        storage_path: Option<PathBuf>,
    ) -> Result<Arc<Self>> {
        let local_ip = get_local_ip().ok_or_else(|| {
            ConnectedError::InitializationError("Could not determine local IP".to_string())
        })?;

        Self::new_with_ip_and_bind(
            device_name,
            device_type,
            port,
            local_ip,
            local_ip,
            storage_path,
        )
        .await
    }

    pub async fn new_with_ip(
        device_name: String,
        device_type: DeviceType,
        port: u16,
        local_ip: IpAddr,
        storage_path: Option<PathBuf>,
    ) -> Result<Arc<Self>> {
        Self::new_with_ip_and_bind(
            device_name,
            device_type,
            port,
            local_ip,
            local_ip,
            storage_path,
        )
        .await
    }

    pub async fn new_with_bind_all(
        device_name: String,
        device_type: DeviceType,
        port: u16,
        storage_path: Option<PathBuf>,
    ) -> Result<Arc<Self>> {
        let local_ip = get_local_ip().unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        let bind_ip = IpAddr::V4(Ipv4Addr::UNSPECIFIED);

        Self::new_with_ip_and_bind(
            device_name,
            device_type,
            port,
            local_ip,
            bind_ip,
            storage_path,
        )
        .await
    }

    pub async fn new_with_bind_ip(
        device_name: String,
        device_type: DeviceType,
        port: u16,
        local_ip: IpAddr,
        bind_ip: IpAddr,
        storage_path: Option<PathBuf>,
    ) -> Result<Arc<Self>> {
        Self::new_with_ip_and_bind(
            device_name,
            device_type,
            port,
            local_ip,
            bind_ip,
            storage_path,
        )
        .await
    }

    async fn new_with_ip_and_bind(
        device_name: String,
        device_type: DeviceType,
        port: u16,
        local_ip: IpAddr,
        bind_ip: IpAddr,
        storage_path: Option<PathBuf>,
    ) -> Result<Arc<Self>> {
        // Load KeyStore first to get the persisted device_id
        let key_store = Arc::new(RwLock::new(KeyStore::new(storage_path.clone())?));
        let device_id = key_store.read().device_id().to_string();

        let download_dir = if let Some(p) = storage_path {
            p.join("downloads")
        } else {
            dirs::download_dir().unwrap_or_else(std::env::temp_dir)
        };

        if !download_dir.exists() {
            std::fs::create_dir_all(&download_dir).map_err(ConnectedError::Io)?;
        }

        // 1. Initialize Transport (QUIC)
        // If port is 0, OS assigns one. We need to know it for mDNS.
        let bind_addr = SocketAddr::new(bind_ip, port);
        let transport = QuicTransport::new(bind_addr, device_id.clone(), key_store.clone()).await?;
        let actual_port = transport.local_addr()?.port();

        // 2. Initialize Device Info
        let local_device = Device::new(device_id, device_name, local_ip, actual_port, device_type);

        // 3. Initialize Discovery (mDNS)
        let discovery = DiscoveryService::new(local_device.clone())
            .map_err(|e| ConnectedError::InitializationError(e.to_string()))?;

        // 4. Create Event Bus
        let (event_tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);

        let client = Arc::new(Self {
            local_device,
            discovery: Arc::new(discovery),
            transport: Arc::new(transport),
            event_tx,
            key_store,
            download_dir,
            auto_accept_files: Arc::new(AtomicBool::new(false)),
            pairing_mode_handle: Arc::new(RwLock::new(None)),
            pending_transfers: Arc::new(RwLock::new(HashMap::new())),
            pending_handshakes: Arc::new(RwLock::new(HashSet::new())),
            fs_provider: Arc::new(RwLock::new(None)),
        });

        // 5. Start Background Tasks
        client.start_background_tasks().await?;

        // 6. Announce Presence
        client.discovery.announce()?;

        Ok(client)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ConnectedEvent> {
        self.event_tx.subscribe()
    }

    pub fn local_device(&self) -> &Device {
        &self.local_device
    }

    pub fn get_discovered_devices(&self) -> Vec<Device> {
        self.discovery.get_discovered_devices()
    }

    pub fn clear_discovered_devices(&self) {
        self.discovery.clear_discovered_devices()
    }

    pub fn inject_proximity_device(
        &self,
        device_id: String,
        device_name: String,
        device_type: DeviceType,
        ip: IpAddr,
        port: u16,
    ) -> Result<()> {
        if device_id == self.local_device.id {
            return Ok(());
        }

        let device = Device::new(device_id, device_name, ip, port, device_type);
        if let Some(event) = self
            .discovery
            .upsert_device_endpoint(device, DiscoverySource::Proximity)
        {
            match event {
                DiscoveryEvent::DeviceFound(d) => {
                    let _ = self.event_tx.send(ConnectedEvent::DeviceFound(d));
                }
                DiscoveryEvent::DeviceLost(id) => {
                    let _ = self.event_tx.send(ConnectedEvent::DeviceLost(id));
                }
                DiscoveryEvent::Error(msg) => {
                    let _ = self
                        .event_tx
                        .send(ConnectedEvent::Error(format!("Discovery error: {}", msg)));
                }
            }
        }

        Ok(())
    }

    pub fn get_fingerprint(&self) -> String {
        self.key_store.read().fingerprint()
    }

    pub fn set_pairing_mode(&self, enabled: bool) {
        // Cancel any existing pairing mode timeout
        if let Some(handle) = self.pairing_mode_handle.write().take() {
            handle.abort();
        }

        self.key_store.write().set_pairing_mode(enabled);
        let _ = self
            .event_tx
            .send(ConnectedEvent::PairingModeChanged(enabled));

        // If enabling, set up auto-disable timer
        if enabled {
            let key_store = self.key_store.clone();
            let event_tx = self.event_tx.clone();
            let handle = tokio::spawn(async move {
                tokio::time::sleep(PAIRING_MODE_TIMEOUT).await;
                key_store.write().set_pairing_mode(false);
                let _ = event_tx.send(ConnectedEvent::PairingModeChanged(false));
                info!("Pairing mode automatically disabled after timeout");
            });
            *self.pairing_mode_handle.write() = Some(handle);
        }
    }

    pub fn set_auto_accept_files(&self, enabled: bool) {
        self.auto_accept_files.store(enabled, Ordering::SeqCst);
    }

    pub fn register_filesystem_provider(
        &self,
        provider: Box<dyn crate::filesystem::FilesystemProvider>,
    ) {
        *self.fs_provider.write() = Some(provider);
    }

    pub async fn fs_list_dir(
        &self,
        target_ip: IpAddr,
        target_port: u16,
        path: String,
    ) -> Result<Vec<crate::filesystem::FsEntry>> {
        use crate::filesystem::{FilesystemMessage, STREAM_TYPE_FS};

        let addr = SocketAddr::new(target_ip, target_port);
        let (mut send, mut recv) = self.transport.open_stream(addr, STREAM_TYPE_FS).await?;

        let req = FilesystemMessage::ListDirRequest { path };
        crate::file_transfer::send_message(&mut send, &req).await?;

        let resp: FilesystemMessage = crate::file_transfer::recv_message(&mut recv).await?;

        match resp {
            FilesystemMessage::ListDirResponse { entries } => Ok(entries),
            FilesystemMessage::Error { message } => Err(ConnectedError::Protocol(message)),
            _ => Err(ConnectedError::Protocol("Unexpected response".to_string())),
        }
    }

    pub async fn fs_download_file(
        &self,
        target_ip: IpAddr,
        target_port: u16,
        remote_path: String,
        local_path: PathBuf,
    ) -> Result<u64> {
        use crate::filesystem::{FilesystemMessage, STREAM_TYPE_FS};
        use tokio::io::AsyncWriteExt;

        let addr = SocketAddr::new(target_ip, target_port);
        let (mut send, mut recv) = self.transport.open_stream(addr, STREAM_TYPE_FS).await?;

        // 1. Get Metadata to know size (optional, but good for allocation or progress)
        let meta_req = FilesystemMessage::GetMetadataRequest {
            path: remote_path.clone(),
        };
        crate::file_transfer::send_message(&mut send, &meta_req).await?;
        let meta_resp: FilesystemMessage = crate::file_transfer::recv_message(&mut recv).await?;

        let file_size = match meta_resp {
            FilesystemMessage::GetMetadataResponse { entry } => entry.size,
            FilesystemMessage::Error { message } => return Err(ConnectedError::Protocol(message)),
            _ => {
                return Err(ConnectedError::Protocol(
                    "Unexpected metadata response".to_string(),
                ));
            }
        };

        // Create local file
        let mut file = tokio::fs::File::create(&local_path)
            .await
            .map_err(ConnectedError::Io)?;

        let mut offset = 0u64;
        let chunk_size = 1024 * 1024; // 1MB chunks

        while offset < file_size {
            let size = std::cmp::min(chunk_size, file_size - offset);
            let req = FilesystemMessage::ReadFileRequest {
                path: remote_path.clone(),
                offset,
                size,
            };
            crate::file_transfer::send_message(&mut send, &req).await?;

            let resp: FilesystemMessage = crate::file_transfer::recv_message(&mut recv).await?;

            match resp {
                FilesystemMessage::ReadFileResponse { data } => {
                    if data.is_empty() {
                        break;
                    }
                    file.write_all(&data).await.map_err(ConnectedError::Io)?;
                    offset += data.len() as u64;
                }
                FilesystemMessage::Error { message } => {
                    return Err(ConnectedError::Protocol(message));
                }
                _ => return Err(ConnectedError::Protocol("Unexpected response".to_string())),
            }
        }

        file.flush().await.map_err(ConnectedError::Io)?;
        Ok(offset)
    }

    pub async fn fs_get_thumbnail(
        &self,
        target_ip: IpAddr,
        target_port: u16,
        path: String,
    ) -> Result<Vec<u8>> {
        use crate::filesystem::{FilesystemMessage, STREAM_TYPE_FS};

        let addr = SocketAddr::new(target_ip, target_port);
        let (mut send, mut recv) = self.transport.open_stream(addr, STREAM_TYPE_FS).await?;

        let req = FilesystemMessage::GetThumbnailRequest { path };
        crate::file_transfer::send_message(&mut send, &req).await?;

        let resp: FilesystemMessage = crate::file_transfer::recv_message(&mut recv).await?;

        match resp {
            FilesystemMessage::GetThumbnailResponse { data } => Ok(data),
            FilesystemMessage::Error { message } => Err(ConnectedError::Protocol(message)),
            _ => Err(ConnectedError::Protocol("Unexpected response".to_string())),
        }
    }

    pub fn is_auto_accept_files(&self) -> bool {
        self.auto_accept_files.load(Ordering::SeqCst)
    }

    pub fn is_pairing_mode(&self) -> bool {
        self.key_store.read().is_pairing_mode()
    }

    pub fn accept_file_transfer(&self, transfer_id: &str) -> Result<()> {
        let sender = self.pending_transfers.write().remove(transfer_id);
        if let Some(tx) = sender {
            let _ = tx.send(true);
            Ok(())
        } else {
            Err(ConnectedError::Protocol(format!(
                "No pending transfer with id: {}",
                transfer_id
            )))
        }
    }

    pub fn reject_file_transfer(&self, transfer_id: &str) -> Result<()> {
        let sender = self.pending_transfers.write().remove(transfer_id);
        if let Some(tx) = sender {
            let _ = tx.send(false);
            Ok(())
        } else {
            Err(ConnectedError::Protocol(format!(
                "No pending transfer with id: {}",
                transfer_id
            )))
        }
    }

    pub fn trust_device(
        &self,
        fingerprint: String,
        device_id: Option<String>,
        name: String,
    ) -> Result<()> {
        // We don't have the device ID here usually unless we cached the handshake request.
        // For now, we trust by fingerprint. The ID can be updated later or passed if known.
        self.key_store
            .write()
            .trust_peer(fingerprint, device_id, Some(name))
    }

    pub fn block_device(&self, fingerprint: String) -> Result<()> {
        // Get device info before blocking for connection invalidation
        let device_id = self
            .key_store
            .read()
            .get_all_known_peers()
            .iter()
            .find(|p| p.fingerprint == fingerprint)
            .and_then(|p| p.device_id.clone());

        self.key_store.write().block_peer(fingerprint)?;

        // Invalidate any cached connection to this device
        if let Some(did) = device_id {
            self.invalidate_connection_by_device_id(&did);
        }

        Ok(())
    }

    pub fn is_device_trusted(&self, device_id: &str) -> bool {
        self.key_store
            .read()
            .get_trusted_peers()
            .iter()
            .any(|p| p.device_id.as_deref() == Some(device_id))
    }

    pub fn get_trusted_peers(&self) -> Vec<crate::security::PeerInfo> {
        self.key_store.read().get_trusted_peers()
    }

    pub async fn send_ping(&self, target_ip: IpAddr, target_port: u16) -> Result<u64> {
        let addr = SocketAddr::new(target_ip, target_port);
        let rtt = self.transport.send_ping(addr).await?;
        Ok(rtt.as_millis() as u64)
    }

    pub async fn send_handshake(&self, target_ip: IpAddr, target_port: u16) -> Result<()> {
        let addr = SocketAddr::new(target_ip, target_port);

        // Ensure pairing mode is enabled for outgoing handshakes
        if !self.is_pairing_mode() {
            self.set_pairing_mode(true);
        }

        // Track this as a pending handshake so we can auto-trust acks even if pairing mode times out
        self.pending_handshakes.write().insert(target_ip);

        let result = self.send_handshake_internal(addr).await;

        // On success or permanent failure, remove from pending
        // Keep it on timeout so background handler can still process late acks
        match &result {
            Ok(()) | Err(ConnectedError::PairingFailed(_)) => {
                self.pending_handshakes.write().remove(&target_ip);
            }
            Err(ConnectedError::Timeout(_)) => {
                // Keep in pending_handshakes - background handler may receive ack later
                // Schedule cleanup after extended timeout
                let pending = self.pending_handshakes.clone();
                let ip = target_ip;
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(300)).await; // 5 min cleanup
                    pending.write().remove(&ip);
                });
            }
            _ => {
                self.pending_handshakes.write().remove(&target_ip);
            }
        }

        result
    }

    async fn send_handshake_internal(&self, addr: SocketAddr) -> Result<()> {
        let (mut send, mut recv) = self
            .transport
            .open_stream(addr, QuicTransport::STREAM_TYPE_CONTROL)
            .await?;

        // Get fingerprint immediately after connecting, before the connection might close
        let peer_fingerprint = self.transport.get_connection_fingerprint(addr).await;
        if peer_fingerprint.is_none() {
            warn!(
                "Could not get peer fingerprint immediately after connecting to {}",
                addr
            );
        }

        let msg = Message::Handshake {
            device_id: self.local_device.id.clone(),
            device_name: self.local_device.name.clone(),
        };

        let data = serde_json::to_vec(&msg)
            .map_err(|e| ConnectedError::InitializationError(e.to_string()))?;
        let len_bytes = (data.len() as u32).to_be_bytes();

        send.write_all(&len_bytes)
            .await
            .map_err(|e| ConnectedError::Io(e.into()))?;
        send.write_all(&data)
            .await
            .map_err(|e| ConnectedError::Io(e.into()))?;

        // Wait for HandshakeAck with timeout, but also check if trust was established
        // via the background handler (trust confirmation on a different stream)
        let key_store = self.key_store.clone();
        let transport = self.transport.clone();
        let target_addr = addr;

        let ack_result = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            // First, try to read from the stream (the receiver might respond directly)
            let read_result: std::result::Result<Message, ConnectedError> = async {
                let mut len_buf = [0u8; 4];
                recv.read_exact(&mut len_buf).await?;
                let msg_len = u32::from_be_bytes(len_buf) as usize;
                if msg_len > 64 * 1024 {
                    return Err(ConnectedError::Protocol("Message too large".to_string()));
                }
                let mut data = vec![0u8; msg_len];
                recv.read_exact(&mut data).await?;
                let response: Message = serde_json::from_slice(&data)?;
                Ok(response)
            }
            .await;

            match read_result {
                Ok(msg) => return Ok(msg),
                Err(e) => {
                    // Stream closed or error - this is expected when receiver shows pairing dialog
                    // The receiver will send trust confirmation on a NEW stream
                    // We need to wait and periodically check if we've been trusted
                    info!(
                        "Stream closed (receiver likely showing pairing dialog), waiting for trust confirmation: {}",
                        e
                    );
                }
            }

            // Stream closed - now poll periodically to check if trusted via background handler
            let check_interval = Duration::from_millis(500);
            let start = std::time::Instant::now();

            loop {
                // Wait before checking
                tokio::time::sleep(check_interval).await;

                // Check if we've been trusted via background handler
                if let Some(fp) = transport.get_connection_fingerprint(target_addr).await {
                    let ks = key_store.read();
                    if ks.is_trusted(&fp) {
                        // Get actual device info from trusted peers
                        let peer_info = ks.get_trusted_peers()
                            .iter()
                            .find(|p| p.fingerprint == fp)
                            .cloned();
                        drop(ks);

                        let (dev_id, dev_name) = if let Some(info) = peer_info {
                            (
                                info.device_id.unwrap_or_else(|| "unknown".to_string()),
                                info.name.unwrap_or_else(|| "Unknown Device".to_string()),
                            )
                        } else {
                            ("already-trusted".to_string(), "Already Trusted".to_string())
                        };

                        info!("Peer trusted via background handler");
                        return Ok(Message::HandshakeAck {
                            device_id: dev_id,
                            device_name: dev_name,
                        });
                    }
                }

                // Check if overall timeout exceeded
                if start.elapsed() >= HANDSHAKE_TIMEOUT - check_interval {
                    return Err(ConnectedError::Timeout("Handshake timeout".to_string()));
                }
            }
        })
        .await;

        send.finish().map_err(|e| ConnectedError::Io(e.into()))?;

        match ack_result {
            Ok(Ok(Message::HandshakeAck {
                device_id,
                device_name,
            })) => {
                // Emit DeviceFound to refresh UI with trusted status
                let device = Device::new(
                    device_id.clone(),
                    device_name.clone(),
                    addr.ip(),
                    addr.port(),
                    DeviceType::Unknown,
                );
                let _ = self.event_tx.send(ConnectedEvent::DeviceFound(device));

                // If already trusted via background handler, just return success
                if device_id == "already-trusted" {
                    info!("Pairing completed (via background trust confirmation)");
                    return Ok(());
                }

                // Use the fingerprint we captured at the start of the handshake
                // This is more reliable than trying to get it after the ack
                let fingerprint = if peer_fingerprint.is_some() {
                    peer_fingerprint.clone()
                } else {
                    // Fallback: try to get it again (might work if connection still open)
                    warn!("Using fallback fingerprint lookup for {}", addr);
                    self.transport.get_connection_fingerprint(addr).await
                };

                if let Some(fp) = fingerprint {
                    self.key_store.write().trust_peer(
                        fp,
                        Some(device_id.clone()),
                        Some(device_name.clone()),
                    )?;
                    info!("Pairing completed with {} ({})", device_name, device_id);

                    // Notify UI
                    let device = Device::new(
                        device_id,
                        device_name,
                        addr.ip(),
                        addr.port(),
                        DeviceType::Unknown,
                    );
                    let _ = self.event_tx.send(ConnectedEvent::DeviceFound(device));
                } else {
                    error!("Failed to get fingerprint for {}, trust not saved!", addr);
                    return Err(ConnectedError::PairingFailed(
                        "Could not obtain peer fingerprint".to_string(),
                    ));
                }
                Ok(())
            }
            Ok(Ok(_)) => Err(ConnectedError::PairingFailed(
                "Unexpected response".to_string(),
            )),
            Ok(Err(e)) => Err(ConnectedError::PairingFailed(e.to_string())),
            Err(_) => {
                // Final check - maybe trust was established just before timeout
                if let Some(fp) = self.transport.get_connection_fingerprint(addr).await {
                    if self.key_store.read().is_trusted(&fp) {
                        info!("Pairing completed (trust established at timeout boundary)");
                        return Ok(());
                    }
                }
                Err(ConnectedError::Timeout("Handshake timeout".to_string()))
            }
        }
    }

    /// Send a HandshakeAck to confirm trust after user approves a pairing request.
    /// This is used instead of send_handshake when responding to an incoming pairing request.
    /// This way, the initiator receives an Ack (not a new Handshake) and both sides are paired.
    pub async fn send_trust_confirmation(&self, target_ip: IpAddr, target_port: u16) -> Result<()> {
        let addr = SocketAddr::new(target_ip, target_port);

        let (mut send, _recv) = self
            .transport
            .open_stream(addr, QuicTransport::STREAM_TYPE_CONTROL)
            .await?;

        let msg = Message::HandshakeAck {
            device_id: self.local_device.id.clone(),
            device_name: self.local_device.name.clone(),
        };

        let data = serde_json::to_vec(&msg)
            .map_err(|e| ConnectedError::InitializationError(e.to_string()))?;
        let len_bytes = (data.len() as u32).to_be_bytes();

        send.write_all(&len_bytes)
            .await
            .map_err(|e| ConnectedError::Io(e.into()))?;
        send.write_all(&data)
            .await
            .map_err(|e| ConnectedError::Io(e.into()))?;
        send.finish().map_err(|e| ConnectedError::Io(e.into()))?;

        info!("Sent trust confirmation to {}:{}", target_ip, target_port);
        Ok(())
    }

    /// Send unpair notification to a device
    pub async fn send_unpair_notification(
        &self,
        target_ip: IpAddr,
        target_port: u16,
        reason: UnpairReason,
    ) -> Result<()> {
        let addr = SocketAddr::new(target_ip, target_port);

        let (mut send, _recv) = match self
            .transport
            .open_stream(addr, QuicTransport::STREAM_TYPE_CONTROL)
            .await
        {
            Ok(s) => s,
            Err(e) => {
                warn!("Could not send unpair notification: {}", e);
                return Ok(()); // Don't fail the unpair if we can't notify
            }
        };

        let msg = Message::DeviceUnpaired {
            device_id: self.local_device.id.clone(),
            reason,
        };

        let data = serde_json::to_vec(&msg)
            .map_err(|e| ConnectedError::InitializationError(e.to_string()))?;
        let len_bytes = (data.len() as u32).to_be_bytes();

        let _ = send.write_all(&len_bytes).await;
        let _ = send.write_all(&data).await;
        let _ = send.finish();

        Ok(())
    }

    pub async fn send_clipboard(
        &self,
        target_ip: IpAddr,
        target_port: u16,
        text: String,
    ) -> Result<()> {
        let addr = SocketAddr::new(target_ip, target_port);
        let (mut send, _recv) = self
            .transport
            .open_stream(addr, QuicTransport::STREAM_TYPE_CONTROL)
            .await?;

        let msg = Message::Clipboard { text };

        let data = serde_json::to_vec(&msg)
            .map_err(|e| ConnectedError::InitializationError(e.to_string()))?;
        let len_bytes = (data.len() as u32).to_be_bytes();

        send.write_all(&len_bytes)
            .await
            .map_err(|e| ConnectedError::Io(e.into()))?;
        send.write_all(&data)
            .await
            .map_err(|e| ConnectedError::Io(e.into()))?;
        send.finish().map_err(|e| ConnectedError::Io(e.into()))?;

        Ok(())
    }

    pub async fn send_media_control(
        &self,
        target_ip: IpAddr,
        target_port: u16,
        msg: crate::transport::MediaControlMessage,
    ) -> Result<()> {
        let addr = SocketAddr::new(target_ip, target_port);
        let (mut send, _recv) = self
            .transport
            .open_stream(addr, QuicTransport::STREAM_TYPE_CONTROL)
            .await?;

        let msg = Message::MediaControl(msg);

        let data = serde_json::to_vec(&msg)
            .map_err(|e| ConnectedError::InitializationError(e.to_string()))?;
        let len_bytes = (data.len() as u32).to_be_bytes();

        send.write_all(&len_bytes)
            .await
            .map_err(|e| ConnectedError::Io(e.into()))?;
        send.write_all(&data)
            .await
            .map_err(|e| ConnectedError::Io(e.into()))?;
        send.finish().map_err(|e| ConnectedError::Io(e.into()))?;

        Ok(())
    }

    pub async fn send_telephony(
        &self,
        target_ip: IpAddr,
        target_port: u16,
        msg: crate::telephony::TelephonyMessage,
    ) -> Result<()> {
        let addr = SocketAddr::new(target_ip, target_port);

        // Try to send, and if it fails due to connection issues, invalidate cache and retry once
        let result = self.send_telephony_inner(addr, &msg).await;

        if let Err(ref e) = result {
            // Check if this is a connection error that might be due to stale connection
            let should_retry = matches!(
                e,
                ConnectedError::Timeout(_) | ConnectedError::Connection(_) | ConnectedError::Io(_)
            );

            if should_retry {
                info!(
                    "Telephony send to {} failed ({}), invalidating connection and retrying...",
                    addr, e
                );
                self.transport.invalidate_connection(&addr);

                // Retry once with fresh connection
                return self.send_telephony_inner(addr, &msg).await;
            }
        }

        result
    }

    async fn send_telephony_inner(
        &self,
        addr: SocketAddr,
        msg: &crate::telephony::TelephonyMessage,
    ) -> Result<()> {
        let (mut send, _recv) = self
            .transport
            .open_stream(addr, QuicTransport::STREAM_TYPE_CONTROL)
            .await?;

        let msg = Message::Telephony(msg.clone());

        let data = serde_json::to_vec(&msg)
            .map_err(|e| ConnectedError::InitializationError(e.to_string()))?;
        let len_bytes = (data.len() as u32).to_be_bytes();

        send.write_all(&len_bytes)
            .await
            .map_err(|e| ConnectedError::Io(e.into()))?;
        send.write_all(&data)
            .await
            .map_err(|e| ConnectedError::Io(e.into()))?;
        send.finish().map_err(|e| ConnectedError::Io(e.into()))?;

        Ok(())
    }

    pub async fn send_file(
        &self,
        target_ip: IpAddr,
        target_port: u16,
        file_path: PathBuf,
    ) -> Result<()> {
        if !file_path.exists() {
            return Err(ConnectedError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "File not found",
            )));
        }

        let addr = SocketAddr::new(target_ip, target_port);
        let connection = self.transport.connect_allow_unknown(addr).await?;

        let event_tx = self.event_tx.clone();
        let transfer_id = uuid::Uuid::new_v4().to_string();
        let peer_name = target_ip.to_string(); // In real app, look up name from discovery

        tokio::spawn(async move {
            let (progress_tx, progress_rx) = mpsc::unbounded_channel();
            let mut progress_rx = progress_rx;

            // Bridge internal progress to public event bus
            let tid = transfer_id.clone();
            let p_peer = peer_name.clone();
            let p_tx = event_tx.clone();

            tokio::spawn(async move {
                while let Some(progress) = progress_rx.recv().await {
                    let event = match progress {
                        TransferProgress::Pending { .. } => {
                            // Pending is only used for incoming transfers, skip for outgoing
                            continue;
                        }
                        TransferProgress::Starting {
                            filename,
                            total_size,
                        } => ConnectedEvent::TransferStarting {
                            id: tid.clone(),
                            filename,
                            total_size,
                            peer_device: p_peer.clone(),
                            direction: TransferDirection::Outgoing,
                        },
                        TransferProgress::Progress {
                            bytes_transferred,
                            total_size,
                        } => ConnectedEvent::TransferProgress {
                            id: tid.clone(),
                            bytes_transferred,
                            total_size,
                        },
                        TransferProgress::Completed { filename, .. } => {
                            ConnectedEvent::TransferCompleted {
                                id: tid.clone(),
                                filename,
                            }
                        }
                        TransferProgress::Failed { error } => ConnectedEvent::TransferFailed {
                            id: tid.clone(),
                            error,
                        },
                        TransferProgress::Cancelled => ConnectedEvent::TransferFailed {
                            id: tid.clone(),
                            error: "Cancelled".into(),
                        },
                    };
                    let _ = p_tx.send(event);
                }
            });

            let file_transfer = FileTransfer::new(connection);
            if let Err(e) = file_transfer.send_file(&file_path, Some(progress_tx)).await {
                let _ = event_tx.send(ConnectedEvent::TransferFailed {
                    id: transfer_id,
                    error: e.to_string(),
                });
            }
        });

        Ok(())
    }

    /// Disconnect from a device but keep it trusted.
    /// Next time they connect, they're still trusted - no need to pair again.
    pub fn disconnect_device(&self, fingerprint: &str) -> Result<()> {
        // Get device info for connection invalidation
        let device_id = self
            .key_store
            .read()
            .get_trusted_peers()
            .iter()
            .find(|p| p.fingerprint == fingerprint)
            .and_then(|p| p.device_id.clone());

        // Only invalidate the connection, DO NOT remove trust
        if let Some(did) = device_id {
            self.invalidate_connection_by_device_id(&did);
        }

        info!("Disconnected from device (trust preserved)");
        Ok(())
    }

    /// Disconnect from a device by ID but keep it trusted.
    pub fn disconnect_device_by_id(&self, device_id: &str) -> Result<()> {
        // Only invalidate the connection, DO NOT remove trust
        self.invalidate_connection_by_device_id(device_id);
        info!("Disconnected from device {} (trust preserved)", device_id);
        Ok(())
    }

    /// Completely remove a peer from trusted list.
    /// This is what "forget" should do - removes trust entirely.
    pub fn remove_trusted_peer(&self, fingerprint: &str) -> Result<()> {
        // Get device info before removing for connection invalidation
        let device_id = self
            .key_store
            .read()
            .get_trusted_peers()
            .iter()
            .find(|p| p.fingerprint == fingerprint)
            .and_then(|p| p.device_id.clone());

        self.key_store.write().remove_peer(fingerprint)?;

        // Invalidate any cached connection to this device
        if let Some(did) = device_id {
            self.invalidate_connection_by_device_id(&did);
        }

        // Emit event so UI updates
        let _ = self
            .event_tx
            .send(ConnectedEvent::PairingModeChanged(self.is_pairing_mode()));
        Ok(())
    }

    /// Completely remove a peer by device_id from trusted list.
    pub fn remove_trusted_peer_by_id(&self, device_id: &str) -> Result<()> {
        self.key_store.write().remove_peer_by_id(device_id)?;

        // Invalidate any cached connection to this device
        self.invalidate_connection_by_device_id(device_id);

        // Emit event so UI updates
        let _ = self
            .event_tx
            .send(ConnectedEvent::PairingModeChanged(self.is_pairing_mode()));
        Ok(())
    }

    /// Invalidate cached connection for a device by looking up its IP from discovered devices
    fn invalidate_connection_by_device_id(&self, device_id: &str) {
        let discovered = self.discovery.get_discovered_devices();
        info!(
            "Looking for device {} in {} discovered devices",
            device_id,
            discovered.len()
        );
        for d in &discovered {
            info!("  - discovered device: id={}, ip={}", d.id, d.ip);
        }

        if let Some(device) = discovered.iter().find(|d| d.id == device_id) {
            if let Some(ip) = device.ip_addr() {
                let addr = SocketAddr::new(ip, device.port);
                self.transport.invalidate_connection(&addr);
                info!(
                    "Invalidated connection cache for device {} at {}",
                    device_id, addr
                );
            } else {
                warn!(
                    "Could not parse IP address '{}' for device {}",
                    device.ip, device_id
                );
            }
        } else {
            warn!(
                "Device {} not found in discovered devices, cannot invalidate connection",
                device_id
            );
        }
    }

    /// Check if an IP has a pending outgoing handshake
    pub fn has_pending_handshake(&self, ip: &IpAddr) -> bool {
        self.pending_handshakes.read().contains(ip)
    }

    /// Forget a device - completely removes trust.
    /// The device will need to go through full pairing request flow again.
    /// This is what the UI "Forget" action should call.
    pub fn forget_device(&self, fingerprint: &str) -> Result<()> {
        // Get device info before removing for connection invalidation
        let device_id = self
            .key_store
            .read()
            .get_trusted_peers()
            .iter()
            .find(|p| p.fingerprint == fingerprint)
            .and_then(|p| p.device_id.clone());

        // Completely remove from keystore (not just set to Forgotten status)
        self.key_store.write().remove_peer(fingerprint)?;

        // Invalidate any cached connection to this device
        if let Some(did) = device_id {
            self.invalidate_connection_by_device_id(&did);
        }

        // Emit event so UI updates
        let _ = self
            .event_tx
            .send(ConnectedEvent::PairingModeChanged(self.is_pairing_mode()));

        info!("Forgot device - trust completely removed");
        Ok(())
    }

    /// Forget a device by its device_id - completely removes trust.
    pub fn forget_device_by_id(&self, device_id: &str) -> Result<()> {
        self.key_store.write().remove_peer_by_id(device_id)?;

        // Invalidate any cached connection to this device
        self.invalidate_connection_by_device_id(device_id);

        // Emit event so UI updates
        let _ = self
            .event_tx
            .send(ConnectedEvent::PairingModeChanged(self.is_pairing_mode()));

        info!("Forgot device {} - trust completely removed", device_id);
        Ok(())
    }

    /// Completely remove a device from known peers
    /// This allows re-pairing without any record (clean slate)
    /// Use remove_trusted_peer or remove_trusted_peer_by_id for this
    ///
    /// Difference between forget and remove:
    /// - forget: Device must go through pairing request (user approval required)
    /// - remove: Device is unknown, can auto-pair in pairing mode
    /// - block: Device cannot connect at all
    pub fn is_device_forgotten(&self, fingerprint: &str) -> bool {
        self.key_store.read().is_forgotten(fingerprint)
    }

    pub fn get_forgotten_peers(&self) -> Vec<crate::security::PeerInfo> {
        self.key_store.read().get_forgotten_peers()
    }

    /// Unpair a device - disconnects but keeps trust intact.
    /// The device can reconnect automatically anytime (no re-pairing needed).
    /// This is what the UI "Unpair" action should call.
    pub fn unpair_device(&self, fingerprint: &str) -> Result<()> {
        // Get device info for connection invalidation
        let device_id = self
            .key_store
            .read()
            .get_trusted_peers()
            .iter()
            .find(|p| p.fingerprint == fingerprint)
            .and_then(|p| p.device_id.clone());

        // Just invalidate the connection - trust remains intact
        if let Some(did) = device_id {
            self.invalidate_connection_by_device_id(&did);
        }

        // Emit event so UI updates
        let _ = self
            .event_tx
            .send(ConnectedEvent::PairingModeChanged(self.is_pairing_mode()));

        info!("Unpaired device - trust preserved, can reconnect anytime");
        Ok(())
    }

    /// Unpair a device by its device_id - disconnects but keeps trust intact.
    pub fn unpair_device_by_id(&self, device_id: &str) -> Result<()> {
        // Just invalidate the connection - trust remains intact
        self.invalidate_connection_by_device_id(device_id);

        // Emit event so UI updates
        let _ = self
            .event_tx
            .send(ConnectedEvent::PairingModeChanged(self.is_pairing_mode()));

        info!(
            "Unpaired device {} - trust preserved, can reconnect anytime",
            device_id
        );
        Ok(())
    }

    pub fn get_blocked_peers(&self) -> Vec<crate::security::PeerInfo> {
        self.key_store.read().get_blocked_peers()
    }

    pub fn get_all_known_peers(&self) -> Vec<crate::security::PeerInfo> {
        self.key_store.read().get_all_known_peers()
    }

    pub async fn send_handshake_ack(&self, send: &mut quinn::SendStream) -> Result<()> {
        let msg = Message::HandshakeAck {
            device_id: self.local_device.id.clone(),
            device_name: self.local_device.name.clone(),
        };

        let data = serde_json::to_vec(&msg)
            .map_err(|e| ConnectedError::InitializationError(e.to_string()))?;
        let len_bytes = (data.len() as u32).to_be_bytes();

        send.write_all(&len_bytes)
            .await
            .map_err(|e| ConnectedError::Io(e.into()))?;
        send.write_all(&data)
            .await
            .map_err(|e| ConnectedError::Io(e.into()))?;

        Ok(())
    }

    async fn start_background_tasks(&self) -> Result<()> {
        // 0. Periodic connection cache cleanup
        let transport_cleanup = self.transport.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                transport_cleanup.cleanup_stale_connections();
            }
        });

        // 0b. Periodic cleanup of stale pending handshakes
        let pending_handshakes_cleanup = self.pending_handshakes.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(300));
            loop {
                interval.tick().await;
                pending_handshakes_cleanup.write().clear();
            }
        });

        // 1. Discovery Listener
        let (disco_tx, mut disco_rx) = mpsc::unbounded_channel();
        self.discovery
            .start_listening(disco_tx)
            .map_err(|e| ConnectedError::Discovery(e.to_string()))?;

        let event_tx = self.event_tx.clone();
        tokio::spawn(async move {
            while let Some(event) = disco_rx.recv().await {
                match event {
                    DiscoveryEvent::DeviceFound(d) => {
                        let _ = event_tx.send(ConnectedEvent::DeviceFound(d));
                    }
                    DiscoveryEvent::DeviceLost(id) => {
                        let _ = event_tx.send(ConnectedEvent::DeviceLost(id));
                    }
                    DiscoveryEvent::Error(msg) => {
                        let _ = event_tx
                            .send(ConnectedEvent::Error(format!("Discovery error: {}", msg)));
                    }
                }
            }
        });

        // 2. Transport Listener (Incoming Messages)
        let (msg_tx, mut msg_rx) = mpsc::unbounded_channel();
        let (file_tx, mut file_rx) = mpsc::unbounded_channel();
        let (fs_tx, mut fs_rx) = mpsc::unbounded_channel();

        self.transport.start_server(msg_tx, file_tx, fs_tx).await?;

        let event_tx = self.event_tx.clone();
        let key_store_control = self.key_store.clone();
        let local_id = self.local_device.id.clone();
        let local_name = self.local_device.name.clone();
        let pending_handshakes = self.pending_handshakes.clone();
        let discovery = self.discovery.clone();

        // Handle Control Messages
        tokio::spawn(async move {
            let key_store = key_store_control;
            while let Some((addr, fingerprint, msg, send_stream)) = msg_rx.recv().await {
                match msg {
                    Message::Handshake {
                        device_id,
                        device_name,
                    } => {
                        let (resolved_name, resolved_type) = discovery
                            .get_device_by_id(&device_id)
                            .map(|d| (d.name, d.device_type))
                            .unwrap_or_else(|| (device_name.clone(), DeviceType::Unknown));

                        let device = Device::new(
                            device_id.clone(),
                            resolved_name,
                            addr.ip(),
                            addr.port(),
                            resolved_type,
                        );

                        if let Some(event) =
                            discovery.upsert_device_endpoint(device, DiscoverySource::Proximity)
                        {
                            match event {
                                DiscoveryEvent::DeviceFound(d) => {
                                    let _ = event_tx.send(ConnectedEvent::DeviceFound(d));
                                }
                                DiscoveryEvent::DeviceLost(id) => {
                                    let _ = event_tx.send(ConnectedEvent::DeviceLost(id));
                                }
                                DiscoveryEvent::Error(msg) => {
                                    let _ = event_tx.send(ConnectedEvent::Error(format!(
                                        "Discovery error: {}",
                                        msg
                                    )));
                                }
                            }
                        }

                        let is_trusted = key_store.read().is_trusted(&fingerprint);
                        let is_forgotten = key_store.read().is_forgotten(&fingerprint);
                        let _needs_pairing = key_store.read().needs_pairing_request(&fingerprint);

                        // If forgotten, always require re-pairing approval (don't auto-trust)
                        if is_forgotten {
                            info!(
                                "Received Handshake from FORGOTTEN peer (requires re-approval): {} - {}",
                                device_name, fingerprint
                            );
                            let _ = event_tx.send(ConnectedEvent::PairingRequest {
                                fingerprint: fingerprint.clone(),
                                device_name,
                                device_id,
                            });
                            continue;
                        }

                        if !is_trusted {
                            // NEVER auto-trust incoming Handshakes - always require user approval
                            // This is a security measure: just because we're in pairing mode
                            // doesn't mean we should auto-trust anyone who connects to us.
                            // Pairing mode only affects OUR outgoing handshakes being auto-trusted
                            // by the remote side when we receive their HandshakeAck.
                            info!(
                                "Received Handshake from untrusted peer: {} - {} (showing pairing request)",
                                device_name, fingerprint
                            );
                            let _ = event_tx.send(ConnectedEvent::PairingRequest {
                                fingerprint: fingerprint.clone(),
                                device_name,
                                device_id,
                            });
                            // Don't send any response - the trust confirmation will be sent
                            // on a new stream when the user trusts via the UI
                        } else {
                            debug!("Received Handshake from trusted peer: {}", device_name);
                            // Update device_id for this trusted peer if it's missing or changed
                            if let Err(e) = key_store.write().trust_peer(
                                fingerprint.clone(),
                                Some(device_id.clone()),
                                Some(device_name.clone()),
                            ) {
                                error!("Failed to update trusted peer info: {}", e);
                            }

                            // Emit DeviceFound to refresh UI with trusted status
                            let device = Device::new(
                                device_id,
                                device_name.clone(),
                                addr.ip(),
                                addr.port(),
                                DeviceType::Unknown,
                            );
                            let _ = event_tx.send(ConnectedEvent::DeviceFound(device));

                            // Send HandshakeAck to confirm we are connected on the same stream
                            if let Some(mut send) = send_stream {
                                let lid = local_id.clone();
                                let lname = local_name.clone();

                                tokio::spawn(async move {
                                    let msg = Message::HandshakeAck {
                                        device_id: lid,
                                        device_name: lname,
                                    };
                                    if let Ok(data) = serde_json::to_vec(&msg) {
                                        let len_bytes = (data.len() as u32).to_be_bytes();
                                        let _ = send.write_all(&len_bytes).await;
                                        let _ = send.write_all(&data).await;
                                        let _ = send.finish();
                                    }
                                });
                            }
                        }
                    }
                    Message::HandshakeAck {
                        device_id: remote_device_id,
                        device_name,
                    } => {
                        info!("Received HandshakeAck from {}", device_name);

                        let is_trusted = key_store.read().is_trusted(&fingerprint);
                        let is_forgotten = key_store.read().is_forgotten(&fingerprint);
                        let has_pending = pending_handshakes.read().contains(&addr.ip());

                        // If forgotten, require re-approval even for acks
                        if is_forgotten {
                            info!(
                                "Received HandshakeAck from FORGOTTEN peer (requires re-approval): {} - {}",
                                device_name, fingerprint
                            );
                            let _ = event_tx.send(ConnectedEvent::PairingRequest {
                                fingerprint: fingerprint.clone(),
                                device_name,
                                device_id: remote_device_id,
                            });
                            continue;
                        }

                        if !is_trusted {
                            // Auto-trust if:
                            // 1. We're in pairing mode, OR
                            // 2. We have a pending handshake to this IP (we initiated)
                            let should_auto_trust =
                                key_store.read().is_pairing_mode() || has_pending;

                            if should_auto_trust {
                                info!(
                                    "Auto-trusting peer from HandshakeAck: {} - {} (pairing_mode={}, pending={})",
                                    device_name,
                                    fingerprint,
                                    key_store.read().is_pairing_mode(),
                                    has_pending
                                );

                                // Remove from pending handshakes
                                pending_handshakes.write().remove(&addr.ip());

                                if let Err(e) = key_store.write().trust_peer(
                                    fingerprint.clone(),
                                    Some(remote_device_id.clone()),
                                    Some(device_name.clone()),
                                ) {
                                    error!("Failed to auto-trust peer: {}", e);
                                }

                                // Emit DeviceFound to refresh UI
                                use crate::device::DeviceType;
                                let d = Device::new(
                                    remote_device_id,
                                    device_name,
                                    addr.ip(),
                                    addr.port(),
                                    DeviceType::Unknown,
                                );
                                let _ = event_tx.send(ConnectedEvent::DeviceFound(d));
                            } else {
                                info!(
                                    "Received HandshakeAck from {} - ignoring (not in pairing mode, no pending handshake)",
                                    device_name
                                );
                            }
                        } else {
                            if let Err(e) = key_store.write().trust_peer(
                                fingerprint.clone(),
                                Some(remote_device_id.clone()),
                                Some(device_name.clone()),
                            ) {
                                error!("Failed to update trusted peer info on Ack: {}", e);
                            }

                            use crate::device::DeviceType;
                            let d = Device::new(
                                remote_device_id,
                                device_name,
                                addr.ip(),
                                addr.port(),
                                DeviceType::Unknown,
                            );
                            let _ = event_tx.send(ConnectedEvent::DeviceFound(d));
                        }
                    }
                    Message::Clipboard { text } => {
                        let is_trusted = key_store.read().is_trusted(&fingerprint);
                        if is_trusted {
                            let from_device = key_store
                                .read()
                                .get_peer_name(&fingerprint)
                                .unwrap_or_else(|| "Unknown".to_string());
                            let _ = event_tx.send(ConnectedEvent::ClipboardReceived {
                                content: text,
                                from_device,
                            });
                        }
                    }
                    Message::MediaControl(media_msg) => {
                        let is_trusted = key_store.read().is_trusted(&fingerprint);
                        if is_trusted {
                            let from_device = key_store
                                .read()
                                .get_peer_name(&fingerprint)
                                .unwrap_or_else(|| "Unknown".to_string());
                            let _ = event_tx.send(ConnectedEvent::MediaControl {
                                from_device,
                                event: media_msg,
                            });
                        }
                    }
                    Message::Telephony(telephony_msg) => {
                        let is_trusted = key_store.read().is_trusted(&fingerprint);
                        if is_trusted {
                            let from_device = key_store
                                .read()
                                .get_peer_name(&fingerprint)
                                .unwrap_or_else(|| "Unknown".to_string());
                            let _ = event_tx.send(ConnectedEvent::Telephony {
                                from_device,
                                from_ip: addr.ip().to_string(),
                                from_port: addr.port(),
                                message: telephony_msg,
                            });
                        }
                    }
                    Message::DeviceUnpaired { device_id, reason } => {
                        let device_name = key_store
                            .read()
                            .get_peer_name(&fingerprint)
                            .unwrap_or_else(|| device_id.clone());

                        match reason {
                            UnpairReason::Unpaired => {
                                info!("Device {} unpaired (trust preserved)", device_id);
                            }
                            UnpairReason::Forgotten | UnpairReason::Blocked => {
                                if let Err(e) = key_store.write().remove_peer(&fingerprint) {
                                    error!("Failed to remove unpaired peer: {}", e);
                                }
                            }
                        }

                        let _ = event_tx.send(ConnectedEvent::DeviceUnpaired {
                            device_id,
                            device_name,
                            reason,
                        });
                    }
                    _ => {}
                }
            }
        });

        // Handle Filesystem Streams
        let fs_provider_clone = self.fs_provider.clone();
        let key_store_fs = self.key_store.clone();

        tokio::spawn(async move {
            while let Some((fingerprint, mut send, mut recv)) = fs_rx.recv().await {
                let is_trusted = key_store_fs.read().is_trusted(&fingerprint);
                if !is_trusted {
                    error!(
                        "Rejected Filesystem Stream from untrusted peer: {}",
                        fingerprint
                    );
                    continue;
                }

                let provider_ref = fs_provider_clone.clone();

                tokio::spawn(async move {
                    use crate::file_transfer::{recv_message, send_message};
                    use crate::filesystem::FilesystemMessage;

                    loop {
                        let msg: Result<FilesystemMessage> = recv_message(&mut recv).await;
                        match msg {
                            Ok(req) => {
                                let provider = provider_ref.clone();
                                let response = tokio::task::spawn_blocking(move || {
                                    let lock = provider.read();
                                    if let Some(p) = lock.as_ref() {
                                        match req {
                                            FilesystemMessage::ListDirRequest { path } => {
                                                match p.list_dir(&path) {
                                                    Ok(entries) => {
                                                        FilesystemMessage::ListDirResponse {
                                                            entries,
                                                        }
                                                    }
                                                    Err(e) => FilesystemMessage::Error {
                                                        message: e.to_string(),
                                                    },
                                                }
                                            }
                                            FilesystemMessage::ReadFileRequest {
                                                path,
                                                offset,
                                                size,
                                            } => match p.read_file(&path, offset, size) {
                                                Ok(data) => {
                                                    FilesystemMessage::ReadFileResponse { data }
                                                }
                                                Err(e) => FilesystemMessage::Error {
                                                    message: e.to_string(),
                                                },
                                            },
                                            FilesystemMessage::WriteFileRequest {
                                                path,
                                                offset,
                                                data,
                                            } => match p.write_file(&path, offset, &data) {
                                                Ok(bytes) => FilesystemMessage::WriteFileResponse {
                                                    bytes_written: bytes,
                                                },
                                                Err(e) => FilesystemMessage::Error {
                                                    message: e.to_string(),
                                                },
                                            },
                                            FilesystemMessage::GetMetadataRequest { path } => {
                                                match p.get_metadata(&path) {
                                                    Ok(entry) => {
                                                        FilesystemMessage::GetMetadataResponse {
                                                            entry,
                                                        }
                                                    }
                                                    Err(e) => FilesystemMessage::Error {
                                                        message: e.to_string(),
                                                    },
                                                }
                                            }
                                            FilesystemMessage::GetThumbnailRequest { path } => {
                                                match p.get_thumbnail(&path) {
                                                    Ok(data) => {
                                                        FilesystemMessage::GetThumbnailResponse {
                                                            data,
                                                        }
                                                    }
                                                    Err(e) => FilesystemMessage::Error {
                                                        message: e.to_string(),
                                                    },
                                                }
                                            }
                                            _ => FilesystemMessage::Error {
                                                message: "Not implemented".to_string(),
                                            },
                                        }
                                    } else {
                                        FilesystemMessage::Error {
                                            message: "No filesystem provider registered"
                                                .to_string(),
                                        }
                                    }
                                })
                                .await;

                                match response {
                                    Ok(resp_msg) => {
                                        if let Err(e) = send_message(&mut send, &resp_msg).await {
                                            warn!("Failed to send FS response: {}", e);
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        error!("FS Handler panicked: {}", e);
                                        break;
                                    }
                                }
                            }
                            Err(_) => break, // Stream closed
                        }
                    }
                });
            }
        });

        // Handle File Streams
        let event_tx = self.event_tx.clone();
        let download_dir = self.download_dir.clone();
        let key_store_files = self.key_store.clone();

        let auto_accept = self.auto_accept_files.clone();
        let pending_transfers = self.pending_transfers.clone();
        tokio::spawn(async move {
            let key_store = key_store_files;
            while let Some((fingerprint, send, recv)) = file_rx.recv().await {
                let is_trusted = key_store.read().is_trusted(&fingerprint);
                if !is_trusted {
                    info!(
                        "Allowing File Stream from untrusted peer (approval required): {}",
                        fingerprint
                    );
                }

                let event_tx = event_tx.clone();
                let download_dir = download_dir.clone();
                let fingerprint_clone = fingerprint.clone();
                let should_auto_accept = is_trusted && auto_accept.load(Ordering::SeqCst);
                let pending = pending_transfers.clone();
                let peer_name = key_store
                    .read()
                    .get_peer_name(&fingerprint)
                    .unwrap_or_else(|| "Unknown".to_string());

                tokio::spawn(async move {
                    let transfer_id = uuid::Uuid::new_v4().to_string();
                    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();

                    let tid = transfer_id.clone();
                    let p_peer = peer_name.clone();
                    let p_fingerprint = fingerprint_clone.clone();
                    let p_tx = event_tx.clone();

                    // Bridge progress events
                    tokio::spawn(async move {
                        while let Some(p) = progress_rx.recv().await {
                            match p {
                                TransferProgress::Pending {
                                    filename,
                                    total_size,
                                    ..
                                } => {
                                    // Emit TransferRequest for user approval
                                    let _ = p_tx.send(ConnectedEvent::TransferRequest {
                                        id: tid.clone(),
                                        filename,
                                        size: total_size,
                                        from_device: p_peer.clone(),
                                        from_fingerprint: p_fingerprint.clone(),
                                    });
                                }
                                TransferProgress::Starting {
                                    filename,
                                    total_size,
                                } => {
                                    let _ = p_tx.send(ConnectedEvent::TransferStarting {
                                        id: tid.clone(),
                                        filename,
                                        total_size,
                                        peer_device: p_peer.clone(),
                                        direction: TransferDirection::Incoming,
                                    });
                                }
                                TransferProgress::Progress {
                                    bytes_transferred,
                                    total_size,
                                } => {
                                    let _ = p_tx.send(ConnectedEvent::TransferProgress {
                                        id: tid.clone(),
                                        bytes_transferred,
                                        total_size,
                                    });
                                }
                                TransferProgress::Completed { filename, .. } => {
                                    let _ = p_tx.send(ConnectedEvent::TransferCompleted {
                                        id: tid.clone(),
                                        filename,
                                    });
                                }
                                TransferProgress::Failed { error } => {
                                    let _ = p_tx.send(ConnectedEvent::TransferFailed {
                                        id: tid.clone(),
                                        error,
                                    });
                                }
                                _ => {}
                            }
                        }
                    });

                    // Create accept channel for user confirmation if not auto-accepting
                    let accept_rx = if !should_auto_accept {
                        let (tx, rx) = oneshot::channel();
                        pending.write().insert(transfer_id.clone(), tx);
                        Some(rx)
                    } else {
                        None
                    };

                    // Handle incoming file with auto-accept preference
                    if let Err(e) = FileTransfer::handle_incoming(
                        send,
                        recv,
                        download_dir,
                        Some(progress_tx),
                        should_auto_accept,
                        accept_rx,
                    )
                    .await
                    {
                        error!("File receive failed: {}", e);
                        // Clean up pending transfer on error
                        pending.write().remove(&transfer_id);
                        let _ = event_tx.send(ConnectedEvent::TransferFailed {
                            id: transfer_id,
                            error: e.to_string(),
                        });
                    }
                });
            }
        });

        Ok(())
    }
}

impl ConnectedClient {
    pub async fn shutdown(&self) {
        info!("Shutting down ConnectedClient...");

        // Cancel pairing mode timeout if active
        if let Some(handle) = self.pairing_mode_handle.write().take() {
            handle.abort();
        }

        // Shutdown transport (closes connections)
        self.transport.shutdown().await;

        // Shutdown discovery (unregisters mDNS, stops threads)
        self.discovery.shutdown();

        info!("ConnectedClient shutdown complete");
    }

    pub async fn broadcast_clipboard(&self, text: String) -> Result<usize> {
        let devices = self.get_discovered_devices();
        let trusted_peers = self.key_store.read().get_trusted_peers();

        // Create a set of trusted device IDs
        let trusted_ids: std::collections::HashSet<String> = trusted_peers
            .into_iter()
            .filter_map(|p| p.device_id)
            .collect();

        let mut tasks = Vec::new();
        let transport = self.transport.clone();

        for device in devices {
            if trusted_ids.contains(&device.id) {
                if let Some(ip) = device.ip_addr() {
                    let port = device.port;
                    let txt = text.clone();
                    let t = transport.clone();

                    tasks.push(tokio::spawn(async move {
                        let addr = SocketAddr::new(ip, port);
                        match t
                            .open_stream(addr, QuicTransport::STREAM_TYPE_CONTROL)
                            .await
                        {
                            Ok((mut send, _recv)) => {
                                let msg = Message::Clipboard { text: txt };
                                if let Ok(data) = serde_json::to_vec(&msg) {
                                    let len_bytes = (data.len() as u32).to_be_bytes();
                                    // Ignore errors during broadcast to keep going
                                    let _ = send.write_all(&len_bytes).await;
                                    let _ = send.write_all(&data).await;
                                    let _ = send.finish(); // Don't await finish to avoid hanging if peer doesn't ack immediately (though for QUIC stream finish is usually fast)
                                    return true;
                                }
                            }
                            Err(e) => {
                                debug!("Failed to send clipboard to {}: {}", addr, e);
                            }
                        }
                        false
                    }));
                }
            }
        }

        let mut sent_count = 0;
        for task in tasks {
            if let Ok(success) = task.await {
                if success {
                    sent_count += 1;
                }
            }
        }
        Ok(sent_count)
    }
}

fn get_local_ip() -> Option<IpAddr> {
    let ifaces = if_addrs::get_if_addrs().ok()?;

    // Filter for IPv4 and non-loopback
    let ipv4_ifaces: Vec<_> = ifaces
        .into_iter()
        .filter(|iface| !iface.is_loopback() && iface.ip().is_ipv4())
        .collect();

    // Try to find a private IP first
    ipv4_ifaces
        .iter()
        .find(|iface| {
            if let IpAddr::V4(ipv4) = iface.ip() {
                let octets = ipv4.octets();
                match octets[0] {
                    10 => true,
                    172 => octets[1] >= 16 && octets[1] <= 31,
                    192 => octets[1] == 168,
                    _ => false,
                }
            } else {
                false
            }
        })
        .map(|iface| iface.ip())
        .or_else(|| {
            // Fallback to any IPv4
            ipv4_ifaces.first().map(|iface| iface.ip())
        })
}
