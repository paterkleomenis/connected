use crate::device::{Device, DeviceType};
use crate::discovery::{DiscoveryEvent, DiscoveryService};
use crate::error::{ConnectedError, Result};
use crate::events::{ConnectedEvent, TransferDirection};
use crate::file_transfer::{FileTransfer, FileTransferMessage, TransferProgress};
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
        let device_id = uuid::Uuid::new_v4().to_string(); // In reality, we should load this from persistence too!

        // Load KeyStore
        let key_store = Arc::new(RwLock::new(KeyStore::new(storage_path.clone())?));

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

    pub fn trust_device(&self, fingerprint: String, name: String) -> Result<()> {
        // We don't have the device ID here usually unless we cached the handshake request.
        // For now, we trust by fingerprint. The ID can be updated later or passed if known.
        self.key_store
            .write()
            .trust_peer(fingerprint, None, Some(name))
    }

    pub fn block_device(&self, fingerprint: String) -> Result<()> {
        self.key_store.write().block_peer(fingerprint)
    }

    pub async fn send_ping(&self, target_ip: IpAddr, target_port: u16) -> Result<u64> {
        let addr = SocketAddr::new(target_ip, target_port);
        let rtt = self.transport.send_ping(addr).await?;
        Ok(rtt.as_millis() as u64)
    }

    pub async fn send_clipboard(
        &self,
        target_ip: IpAddr,
        target_port: u16,
        text: String,
    ) -> Result<()> {
        let addr = SocketAddr::new(target_ip, target_port);
        let (mut send, mut recv) = self
            .transport
            .open_stream(addr, QuicTransport::STREAM_TYPE_CONTROL)
            .await?;

        let msg = FileTransferMessage::ClipboardText { text };
        // let (mut send, mut recv) = connection.open_bi().await?; // Replaced by open_stream

        // Using transport helper if possible, otherwise duplicating simple send logic
        // We need to use the helpers from file_transfer now since we made them pub(crate)

        crate::file_transfer::send_message(&mut send, &msg).await?;

        // Wait for Ack
        let _: FileTransferMessage = crate::file_transfer::recv_message(&mut recv).await?;

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
        let key_store = self.key_store.clone();

        // Handle Control Messages
        tokio::spawn(async move {
            while let Some((_addr, fingerprint, msg)) = msg_rx.recv().await {
                match msg {
                    Message::Handshake {
                        device_id,
                        device_name,
                    } => {
                        let is_trusted = key_store.read().is_trusted(&fingerprint);
                        if !is_trusted {
                            // It must be in pairing mode then, otherwise PeerVerifier would have rejected it.
                            info!(
                                "Received Handshake from untrusted peer (Pairing Mode): {} - {}",
                                device_name, fingerprint
                            );
                            let _ = event_tx.send(ConnectedEvent::PairingRequest {
                                fingerprint: fingerprint.clone(),
                                device_name,
                                device_id,
                            });
                        } else {
                            debug!("Received Handshake from trusted peer: {}", device_name);
                        }
                    }
                    _ => {}
                }
            }
        });

        // Handle File Streams
        let event_tx = self.event_tx.clone();
        let download_dir = self.download_dir.clone();

        tokio::spawn(async move {
            while let Some((fingerprint, send, recv)) = file_rx.recv().await {
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

fn get_local_ip() -> Option<IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    socket.local_addr().ok().map(|a| a.ip())
}
