use crate::device::{Device, DeviceType};
use crate::discovery::{DiscoveryEvent, DiscoveryService};
use crate::error::{ConnectedError, Result};
use crate::events::{ConnectedEvent, TransferDirection};
use crate::file_transfer::{FileTransfer, TransferProgress};
use crate::security::KeyStore;
use crate::transport::{Message, QuicTransport};
use parking_lot::RwLock;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, info};

const EVENT_CHANNEL_CAPACITY: usize = 100;

pub struct ConnectedClient {
    local_device: Device,
    discovery: Arc<DiscoveryService>,
    transport: Arc<QuicTransport>,
    event_tx: broadcast::Sender<ConnectedEvent>,
    key_store: Arc<RwLock<KeyStore>>,
    download_dir: PathBuf,
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

        Self::new_with_ip(device_name, device_type, port, local_ip, storage_path).await
    }

    pub async fn new_with_ip(
        device_name: String,
        device_type: DeviceType,
        port: u16,
        local_ip: IpAddr,
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
        let bind_addr = SocketAddr::new(local_ip, port);
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

    pub fn get_fingerprint(&self) -> String {
        self.key_store.read().fingerprint()
    }

    pub fn set_pairing_mode(&self, enabled: bool) {
        self.key_store.write().set_pairing_mode(enabled);
        let _ = self
            .event_tx
            .send(ConnectedEvent::PairingModeChanged(enabled));
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
        self.key_store.write().block_peer(fingerprint)
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
        let (mut send, _recv) = self
            .transport
            .open_stream(addr, QuicTransport::STREAM_TYPE_CONTROL)
            .await?;

        let msg = Message::Handshake {
            device_id: self.local_device.id.clone(),
            device_name: self.local_device.name.clone(),
        };

        let data = serde_json::to_vec(&msg)
            .map_err(|e| ConnectedError::InitializationError(e.to_string()))?;
        let len_bytes = (data.len() as u32).to_be_bytes();

        // Write Stream Type (Already handled by open_stream? No, open_stream writes the type byte!)
        // Wait, check QuicTransport::open_stream in transport.rs
        // It does: send.write_all(&[stream_type]).await?;
        // So we don't write the type byte here.

        // Write Length
        send.write_all(&len_bytes)
            .await
            .map_err(|e| ConnectedError::Io(e.into()))?;
        // Write Data
        send.write_all(&data)
            .await
            .map_err(|e| ConnectedError::Io(e.into()))?;
        send.finish().map_err(|e| ConnectedError::Io(e.into()))?;

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
        let connection = self.transport.connect(addr).await?;

        let event_tx = self.event_tx.clone();
        let transfer_id = uuid::Uuid::new_v4().to_string();
        let peer_name = target_ip.to_string(); // In real app, look up name from discovery

        tokio::spawn(async move {
            let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();

            // Bridge internal progress to public event bus
            let tid = transfer_id.clone();
            let p_peer = peer_name.clone();
            let p_tx = event_tx.clone();

            tokio::spawn(async move {
                while let Some(progress) = progress_rx.recv().await {
                    let event = match progress {
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

    pub fn remove_trusted_peer(&self, fingerprint: &str) -> Result<()> {
        self.key_store.write().remove_peer(fingerprint)
    }

    pub fn remove_trusted_peer_by_id(&self, device_id: &str) -> Result<()> {
        self.key_store.write().remove_peer_by_id(device_id)
    }

    pub async fn send_handshake_ack(&self, target_ip: IpAddr, target_port: u16) -> Result<()> {
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

        Ok(())
    }

    async fn start_background_tasks(&self) -> Result<()> {
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

        self.transport.start_server(msg_tx, file_tx).await?;

        let event_tx = self.event_tx.clone();
        let key_store_control = self.key_store.clone();
        let transport_control = self.transport.clone();
        let local_id = self.local_device.id.clone();
        let local_name = self.local_device.name.clone();

        // Handle Control Messages
        tokio::spawn(async move {
            let key_store = key_store_control;
            while let Some((addr, fingerprint, msg)) = msg_rx.recv().await {
                match msg {
                    Message::Handshake {
                        device_id,
                        device_name,
                    } => {
                        let is_trusted = key_store.read().is_trusted(&fingerprint);
                        if !is_trusted {
                            // If we are in pairing mode, we initiated or are expecting a connection.
                            // We should accept this Handshake as confirmation.
                            if key_store.read().is_pairing_mode() {
                                info!(
                                    "Auto-trusting peer from Handshake (Pairing Mode): {} - {}",
                                    device_name, fingerprint
                                );
                                if let Err(e) = key_store.write().trust_peer(
                                    fingerprint.clone(),
                                    Some(device_id.clone()),
                                    Some(device_name.clone()),
                                ) {
                                    error!("Failed to auto-trust peer: {}", e);
                                }

                                // Send Ack back
                                let t = transport_control.clone();
                                let lid = local_id.clone();
                                let lname = local_name.clone();

                                tokio::spawn(async move {
                                    let msg = Message::HandshakeAck {
                                        device_id: lid,
                                        device_name: lname,
                                    };
                                    if let Ok((mut send, _recv)) = t
                                        .open_stream(addr, QuicTransport::STREAM_TYPE_CONTROL)
                                        .await
                                    {
                                        if let Ok(data) = serde_json::to_vec(&msg) {
                                            let len_bytes = (data.len() as u32).to_be_bytes();
                                            let _ = send.write_all(&len_bytes).await;
                                            let _ = send.write_all(&data).await;
                                            let _ = send.finish();
                                        }
                                    }
                                });

                                // Notify UI
                                use crate::device::DeviceType;
                                let d = Device::new(
                                    device_id,
                                    device_name,
                                    addr.ip(),
                                    addr.port(),
                                    DeviceType::Unknown,
                                );
                                let _ = event_tx.send(ConnectedEvent::DeviceFound(d));
                            } else {
                                // Passive side: We are not in pairing mode, so this is a new unsolicited request.
                                info!(
                                    "Received Handshake from untrusted peer (Passive): {} - {}",
                                    device_name, fingerprint
                                );
                                let _ = event_tx.send(ConnectedEvent::PairingRequest {
                                    fingerprint: fingerprint.clone(),
                                    device_name,
                                    device_id,
                                });
                            }
                        } else {
                            debug!("Received Handshake from trusted peer: {}", device_name);
                            // Update device_id for this trusted peer if it's missing or changed
                            if let Err(e) = key_store.write().trust_peer(
                                fingerprint.clone(),
                                Some(device_id),
                                Some(device_name.clone()),
                            ) {
                                error!("Failed to update trusted peer info: {}", e);
                            }

                            // Send HandshakeAck to confirm we are connected
                            let t = transport_control.clone();
                            let lid = local_id.clone();
                            let lname = local_name.clone();

                            tokio::spawn(async move {
                                let msg = Message::HandshakeAck {
                                    device_id: lid,
                                    device_name: lname,
                                };
                                if let Ok((mut send, _recv)) = t
                                    .open_stream(addr, QuicTransport::STREAM_TYPE_CONTROL)
                                    .await
                                {
                                    if let Ok(data) = serde_json::to_vec(&msg) {
                                        let len_bytes = (data.len() as u32).to_be_bytes();
                                        let _ = send.write_all(&len_bytes).await;
                                        let _ = send.write_all(&data).await;
                                        let _ = send.finish();
                                    }
                                }
                            });
                        }
                    }
                    Message::HandshakeAck {
                        device_id: remote_device_id,
                        device_name,
                    } => {
                        info!("Received HandshakeAck from {}", device_name);

                        let is_trusted = key_store.read().is_trusted(&fingerprint);
                        if !is_trusted {
                            // If we initiated pairing (pairing_mode = true) and they Acked,
                            // we should Auto-Trust them because we asked for it.
                            if key_store.read().is_pairing_mode() {
                                info!(
                                    "Auto-trusting peer from HandshakeAck (Pairing Mode): {} - {}",
                                    device_name, fingerprint
                                );
                                if let Err(e) = key_store.write().trust_peer(
                                    fingerprint.clone(),
                                    Some(remote_device_id.clone()),
                                    Some(device_name.clone()),
                                ) {
                                    error!("Failed to auto-trust peer: {}", e);
                                }

                                // Turn off pairing mode now that we succeeded?
                                // Maybe keep it on briefly or let UI handle it.
                                // But definitely don't emit PairingRequest.

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
                                // We are NOT in pairing mode, but they sent an Ack?
                                // This is weird. Maybe we forgot them?
                                // Treat as request.
                                info!("Received HandshakeAck from untrusted peer (Not in Pairing Mode): {} - {}", device_name, fingerprint);
                                let _ = event_tx.send(ConnectedEvent::PairingRequest {
                                    fingerprint: fingerprint.clone(),
                                    device_name,
                                    device_id: remote_device_id,
                                });
                            }
                        } else {
                            // Already trusted. Update info just in case (e.g. name change or ID sync)
                            if let Err(e) = key_store.write().trust_peer(
                                fingerprint.clone(),
                                Some(remote_device_id.clone()),
                                Some(device_name.clone()),
                            ) {
                                error!("Failed to update trusted peer info on Ack: {}", e);
                            }

                            // Force UI refresh
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
                            // Find device name if possible
                            let from_device = key_store
                                .read()
                                .get_peer_name(&fingerprint)
                                .unwrap_or_else(|| "Unknown".to_string());
                            let _ = event_tx.send(ConnectedEvent::ClipboardReceived {
                                content: text,
                                from_device,
                            });
                        } else {
                            error!("Rejected Clipboard from untrusted peer: {}", fingerprint);
                        }
                    }
                    _ => {}
                }
            }
        });

        // Handle File Streams
        let event_tx = self.event_tx.clone();
        let download_dir = self.download_dir.clone();
        let key_store_files = self.key_store.clone();

        tokio::spawn(async move {
            let key_store = key_store_files;
            while let Some((fingerprint, send, recv)) = file_rx.recv().await {
                let is_trusted = key_store.read().is_trusted(&fingerprint);
                if !is_trusted {
                    error!("Rejected File Stream from untrusted peer: {}", fingerprint);
                    // We can't easily close just this stream without dropping send/recv,
                    // which happens when we continue loop (they go out of scope).
                    // Sending an error frame might be polite but strict drop is safer.
                    continue;
                }

                let event_tx = event_tx.clone();
                let download_dir = download_dir.clone();
                let fingerprint = fingerprint.clone();

                tokio::spawn(async move {
                    let transfer_id = uuid::Uuid::new_v4().to_string();
                    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();

                    let tid = transfer_id.clone();
                    let p_peer = fingerprint.clone();
                    let p_tx = event_tx.clone();

                    // Bridge progress events
                    tokio::spawn(async move {
                        while let Some(p) = progress_rx.recv().await {
                            match p {
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

                    // Handle incoming file
                    // We auto-accept for now, or we could check a "auto-receive" preference
                    if let Err(e) = FileTransfer::handle_incoming(
                        send,
                        recv,
                        download_dir,
                        Some(progress_tx),
                        true,
                    )
                    .await
                    {
                        error!("File receive failed: {}", e);
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
    // ... existing methods ...

    pub async fn broadcast_clipboard(&self, text: String) -> Result<usize> {
        let devices = self.get_discovered_devices();
        let trusted_peers = self.key_store.read().get_trusted_peers();

        // Create a set of trusted device IDs
        let trusted_ids: std::collections::HashSet<String> = trusted_peers
            .into_iter()
            .filter_map(|p| p.device_id)
            .collect();

        let mut sent_count = 0;

        for device in devices {
            if trusted_ids.contains(&device.id) {
                if let Some(ip) = device.ip_addr() {
                    let port = device.port;
                    let txt = text.clone();
                    // We need a way to call send_clipboard async without blocking the loop
                    // But send_clipboard is on &self.
                    // We can't easily spawn a task that uses &self unless we clone the Arc holding self.
                    // But here we are inside &self method.
                    // We can assume the caller will handle concurrency if they want,
                    // OR we can just await sequentially which is safer for now to avoid too many connections.
                    // Given it's QUIC, it should be fast.

                    // Actually, let's just do it sequentially for now.
                    if let Err(e) = self.send_clipboard(ip, port, txt).await {
                        error!("Failed to broadcast clipboard to {}: {}", device.name, e);
                    } else {
                        sent_count += 1;
                    }
                }
            }
        }
        Ok(sent_count)
    }
}

fn get_local_ip() -> Option<IpAddr> {
    if_addrs::get_if_addrs()
        .ok()?
        .into_iter()
        .find(|iface| !iface.is_loopback() && iface.ip().is_ipv4())
        .map(|iface| iface.ip())
}
