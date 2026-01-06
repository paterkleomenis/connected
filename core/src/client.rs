use crate::device::{Device, DeviceType};
use crate::discovery::{DiscoveryEvent, DiscoveryService};
use crate::error::{ConnectedError, Result};
use crate::events::{ConnectedEvent, TransferDirection};
use crate::file_transfer::{FileTransfer, FileTransferMessage, TransferProgress};
use crate::transport::{Message, QuicTransport};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info};

const EVENT_CHANNEL_CAPACITY: usize = 100;

pub struct ConnectedClient {
    local_device: Device,
    discovery: Arc<DiscoveryService>,
    transport: Arc<QuicTransport>,
    event_tx: broadcast::Sender<ConnectedEvent>,
    // We keep a handle to the background tasks to abort them on drop if needed
    // (Though simple structure drop is usually enough)
}

impl ConnectedClient {
    pub async fn new(device_name: String, device_type: DeviceType, port: u16) -> Result<Arc<Self>> {
        let local_ip = get_local_ip().ok_or_else(|| {
            ConnectedError::InitializationError("Could not determine local IP".to_string())
        })?;

        Self::new_with_ip(device_name, device_type, port, local_ip).await
    }

    pub async fn new_with_ip(
        device_name: String,
        device_type: DeviceType,
        port: u16,
        local_ip: IpAddr,
    ) -> Result<Arc<Self>> {
        let device_id = uuid::Uuid::new_v4().to_string();

        // 1. Initialize Transport (QUIC)
        // If port is 0, OS assigns one. We need to know it for mDNS.
        let bind_addr = SocketAddr::new(local_ip, port);
        let transport = QuicTransport::new(bind_addr, device_id.clone()).await?;
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
        let connection = self.transport.connect(addr).await?;

        let msg = FileTransferMessage::ClipboardText { text };
        let (mut send, mut recv) = connection.open_bi().await?;

        // Simple manual serialization for control messages
        // (Ideally we should reuse transport send/recv helpers but they are private or need refactor)
        // For now, implementing basic logic here or exposing helpers from transport

        // Using transport helper if possible, otherwise duplicating simple send logic
        let data = serde_json::to_vec(&msg)?;
        let len = (data.len() as u32).to_be_bytes();

        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        send.write_all(&len).await?;
        send.write_all(&data).await?;
        send.finish()?;

        // Wait for Ack
        let mut len_buf = [0u8; 4];
        recv.read_exact(&mut len_buf).await?; // Wait for ack
                                              // We don't strictly need to parse the ack for clipboard, just knowing they got it is enough

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
        self.transport.start_server(msg_tx).await?;

        let event_tx = self.event_tx.clone();
        let download_dir = dirs::download_dir().unwrap_or_else(|| PathBuf::from("."));
        // TODO: Allow configuring download dir

        tokio::spawn(async move {
            while let Some((addr, msg)) = msg_rx.recv().await {
                match msg {
                    Message::FileTransfer => {
                        // This usually means a raw stream connection for file transfer is incoming
                        // But wait, QuicTransport::handle_connection loops and reads messages.
                        // If it's a file transfer, the logic in handle_connection might currently consume the stream?
                        // Let's check transport.rs again.
                        // Actually handle_connection reads messages. For file transfer, we likely need
                        // to handle the stream itself or a specific message that initiates it.
                        // In the previous implementation, start_file_receiver created a SEPARATE endpoint on port+1.
                        // We want to unify this if possible, or support the separate port for now.
                        // For Phase 1, let's keep the logic simple: Transport handles control messages.
                        // But wait, FileTransfer uses the SAME connection.
                    }
                    _ => {}
                }
            }
        });

        // Handle incoming streams for File Transfer / Clipboard
        // The transport.start_server handles accepting connections and loops over bi-streams.
        // We need to hook into that loop.
        // Currently transport.rs consumes the stream to read the first message.
        // We need to modify transport.rs to allow handing off the stream.

        Ok(())
    }

    // Helper to start the separate file receiver (legacy support for now until full multiplexing)
    pub async fn start_file_receiver(&self, save_path: PathBuf) -> Result<()> {
        // This logic was previously in facade.rs using a separate port (port + 1)
        // We should implement this properly.
        // For the MVP refactor, let's reimplement the separate listener here to match previous behavior
        // but connected to the event bus.

        let bind_ip = self.local_device.ip.parse().unwrap();
        let bind_port = self.local_device.port + 1;
        let bind_addr = SocketAddr::new(bind_ip, bind_port);

        let server_config = QuicTransport::create_server_config().map_err(|e| {
            ConnectedError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                e.to_string(),
            ))
        })?;
        let endpoint =
            quinn::Endpoint::server(server_config, bind_addr).map_err(|e| ConnectedError::Io(e))?;

        let event_tx = self.event_tx.clone();

        tokio::spawn(async move {
            while let Some(incoming) = endpoint.accept().await {
                let event_tx = event_tx.clone();
                let save_path = save_path.clone();

                tokio::spawn(async move {
                    if let Ok(connection) = incoming.await {
                        let remote_addr = connection.remote_address();
                        if let Ok((mut send, mut recv)) = connection.accept_bi().await {
                            // Handle the incoming stream
                            // We need to peek/read the first message to know if it's Clipboard or File

                            use tokio::io::AsyncReadExt;
                            // Read length
                            let mut len_buf = [0u8; 4];
                            if recv.read_exact(&mut len_buf).await.is_err() {
                                return;
                            }
                            let len = u32::from_be_bytes(len_buf) as usize;
                            let mut data = vec![0u8; len];
                            if recv.read_exact(&mut data).await.is_err() {
                                return;
                            }

                            if let Ok(msg) = serde_json::from_slice::<FileTransferMessage>(&data) {
                                match msg {
                                    FileTransferMessage::ClipboardText { text } => {
                                        let _ = event_tx.send(ConnectedEvent::ClipboardReceived {
                                            content: text,
                                            from_device: remote_addr.ip().to_string(), // In future resolve to ID
                                        });
                                        // Send Ack
                                        let ack = FileTransferMessage::ClipboardAck;
                                        let ack_data = serde_json::to_vec(&ack).unwrap();
                                        let len = (ack_data.len() as u32).to_be_bytes();
                                        use tokio::io::AsyncWriteExt;
                                        let _ = send.write_all(&len).await;
                                        let _ = send.write_all(&ack_data).await;
                                        let _ = send.finish();
                                    }
                                    FileTransferMessage::SendRequest { filename, size, .. } => {
                                        // Handle file transfer
                                        let transfer_id = uuid::Uuid::new_v4().to_string();
                                        let _ = event_tx.send(ConnectedEvent::TransferStarting {
                                            id: transfer_id.clone(),
                                            filename: filename.clone(),
                                            total_size: size,
                                            peer_device: remote_addr.ip().to_string(),
                                            direction: TransferDirection::Incoming,
                                        });

                                        // Create a "Pre-read" stream wrapper since we already read the first message?
                                        // Or just pass the data to a modified receive_file

                                        // Easier: modify receive_file to accept the already-read request
                                        // But receive_file is in file_transfer.rs and expects to read from stream.
                                        // We can reconstruct the logic here for now or refactor receive_file.
                                        // Let's refactor receive_file to take the request message.

                                        // For now, I'll inline the logic to save time on multiple file edits
                                        let (progress_tx, mut progress_rx) =
                                            mpsc::unbounded_channel();

                                        // Bridge events
                                        let p_tx = event_tx.clone();
                                        let tid = transfer_id.clone();
                                        tokio::spawn(async move {
                                            while let Some(p) = progress_rx.recv().await {
                                                // Map progress to events...
                                                match p {
                                                    TransferProgress::Progress {
                                                        bytes_transferred,
                                                        total_size,
                                                    } => {
                                                        let _ = p_tx.send(
                                                            ConnectedEvent::TransferProgress {
                                                                id: tid.clone(),
                                                                bytes_transferred,
                                                                total_size,
                                                            },
                                                        );
                                                    }
                                                    TransferProgress::Completed {
                                                        filename,
                                                        ..
                                                    } => {
                                                        let _ = p_tx.send(
                                                            ConnectedEvent::TransferCompleted {
                                                                id: tid.clone(),
                                                                filename,
                                                            },
                                                        );
                                                    }
                                                    TransferProgress::Failed { error } => {
                                                        let _ = p_tx.send(
                                                            ConnectedEvent::TransferFailed {
                                                                id: tid.clone(),
                                                                error,
                                                            },
                                                        );
                                                    }
                                                    _ => {}
                                                }
                                            }
                                        });

                                        // Send Accept
                                        let accept = FileTransferMessage::Accept;
                                        let acc_data = serde_json::to_vec(&accept).unwrap();
                                        use tokio::io::AsyncWriteExt;
                                        let _ = send
                                            .write_all(&(acc_data.len() as u32).to_be_bytes())
                                            .await;
                                        let _ = send.write_all(&acc_data).await;

                                        // Read chunks
                                        let safe_filename = sanitize_filename(&filename);
                                        let path = save_path.join(&safe_filename);

                                        if let Ok(file) = tokio::fs::File::create(&path).await {
                                            let mut writer = tokio::io::BufWriter::new(file);
                                            let mut bytes = 0;
                                            let mut hasher = crc32fast::Hasher::new();

                                            loop {
                                                // Read next msg
                                                // Reuse recv logic...
                                                let mut len_buf = [0u8; 4];
                                                if recv.read_exact(&mut len_buf).await.is_err() {
                                                    break;
                                                }
                                                let len = u32::from_be_bytes(len_buf) as usize;
                                                let mut buf = vec![0u8; len];
                                                if recv.read_exact(&mut buf).await.is_err() {
                                                    break;
                                                }

                                                if let Ok(m) =
                                                    serde_json::from_slice::<FileTransferMessage>(
                                                        &buf,
                                                    )
                                                {
                                                    match m {
                                                        FileTransferMessage::Chunk {
                                                            data, ..
                                                        } => {
                                                            hasher.update(&data);
                                                            let _ = writer.write_all(&data).await;
                                                            bytes += data.len() as u64;
                                                            let _ = progress_tx.send(
                                                                TransferProgress::Progress {
                                                                    bytes_transferred: bytes,
                                                                    total_size: size,
                                                                },
                                                            );
                                                        }
                                                        FileTransferMessage::Complete {
                                                            checksum,
                                                        } => {
                                                            let _ = writer.flush().await;
                                                            let our_sum = format!(
                                                                "{:08x}",
                                                                hasher.finalize()
                                                            );
                                                            if our_sum == checksum {
                                                                let _ = progress_tx.send(
                                                                    TransferProgress::Completed {
                                                                        filename: safe_filename
                                                                            .clone(),
                                                                        total_size: size,
                                                                    },
                                                                );
                                                                // Ack
                                                                let ack = FileTransferMessage::Ack;
                                                                let d = serde_json::to_vec(&ack)
                                                                    .unwrap();
                                                                let _ = send
                                                                    .write_all(
                                                                        &(d.len() as u32)
                                                                            .to_be_bytes(),
                                                                    )
                                                                    .await;
                                                                let _ = send.write_all(&d).await;
                                                            } else {
                                                                let _ = progress_tx.send(
                                                                    TransferProgress::Failed {
                                                                        error: "Checksum mismatch"
                                                                            .into(),
                                                                    },
                                                                );
                                                            }
                                                            break;
                                                        }
                                                        _ => {}
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
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

fn sanitize_filename(filename: &str) -> String {
    std::path::Path::new(filename)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unnamed")
        .chars()
        .filter(|c| {
            !matches!(
                c,
                '/' | '\\' | '\0' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
            )
        })
        .take(255)
        .collect()
}
