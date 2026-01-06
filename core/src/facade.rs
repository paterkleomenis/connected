use crate::device::{Device, DeviceType};
use crate::discovery::{DiscoveryEvent, DiscoveryService};
use crate::error::ConnectedError;
use crate::file_transfer::{FileTransfer, FileTransferMessage, TransferProgress};
use crate::transport::QuicTransport;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

// ============================================================================
// Data Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredDevice {
    pub id: String,
    pub name: String,
    pub ip: String,
    pub port: u16,
    pub device_type: String,
}

impl From<Device> for DiscoveredDevice {
    fn from(device: Device) -> Self {
        Self {
            id: device.id,
            name: device.name,
            ip: device.ip.to_string(),
            port: device.port,
            device_type: device.device_type.as_str().to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PingResult {
    pub success: bool,
    pub rtt_ms: u64,
    pub error_message: Option<String>,
}

// ============================================================================
// Traits
// ============================================================================

pub trait DiscoveryCallback: Send + Sync {
    fn on_device_found(&self, device: DiscoveredDevice);
    fn on_device_lost(&self, device_id: String);
    fn on_error(&self, error_msg: String);
}

pub trait FileTransferCallback: Send + Sync {
    fn on_transfer_starting(&self, filename: String, total_size: u64);
    fn on_transfer_progress(&self, bytes_transferred: u64, total_size: u64);
    fn on_transfer_completed(&self, filename: String, total_size: u64);
    fn on_transfer_failed(&self, error_msg: String);
    fn on_transfer_cancelled(&self);
}

pub trait ClipboardCallback: Send + Sync {
    fn on_clipboard_received(&self, text: String, from_device: String);
    fn on_clipboard_sent(&self, success: bool, error_msg: Option<String>);
}

// ============================================================================
// Core Logic
// ============================================================================

struct ConnectedCore {
    discovery: Arc<DiscoveryService>,
    transport: Arc<QuicTransport>,
    local_device: Device,
}

static RUNTIME: OnceLock<Runtime> = OnceLock::new();
static CONNECTED_INSTANCE: OnceLock<RwLock<Option<ConnectedCore>>> = OnceLock::new();
static CLIPBOARD_CALLBACK: OnceLock<RwLock<Option<Arc<dyn ClipboardCallback>>>> = OnceLock::new();

fn get_runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create Tokio runtime")
    })
}

fn get_instance() -> &'static RwLock<Option<ConnectedCore>> {
    CONNECTED_INSTANCE.get_or_init(|| RwLock::new(None))
}

fn get_clipboard_callback() -> &'static RwLock<Option<Arc<dyn ClipboardCallback>>> {
    CLIPBOARD_CALLBACK.get_or_init(|| RwLock::new(None))
}

pub fn initialize(
    device_name: String,
    device_type: String,
    bind_port: u16,
) -> Result<(), ConnectedError> {
    let local_ip = get_local_ip().ok_or(ConnectedError::InitializationError(
        "Could not determine local IP".into(),
    ))?;
    initialize_internal(device_name, device_type, bind_port, local_ip)
}

pub fn initialize_with_ip(
    device_name: String,
    device_type: String,
    bind_port: u16,
    ip_address: String,
) -> Result<(), ConnectedError> {
    let local_ip: IpAddr = ip_address
        .parse()
        .map_err(|_| ConnectedError::InitializationError(format!("Invalid IP: {}", ip_address)))?;
    initialize_internal(device_name, device_type, bind_port, local_ip)
}

fn initialize_internal(
    device_name: String,
    device_type: String,
    bind_port: u16,
    local_ip: IpAddr,
) -> Result<(), ConnectedError> {
    let runtime = get_runtime();
    let device_id = uuid::Uuid::new_v4().to_string();

    let local_device = Device::new(
        device_id,
        device_name,
        local_ip,
        bind_port,
        DeviceType::from_str(&device_type),
    );

    let discovery = DiscoveryService::new(local_device.clone())
        .map_err(|e| ConnectedError::InitializationError(e.to_string()))?;

    // Announce immediately
    discovery.announce()?;

    let bind_addr = SocketAddr::new(local_ip, bind_port);
    let transport =
        runtime.block_on(async { QuicTransport::new(bind_addr, local_device.id.clone()).await })?;

    let core = ConnectedCore {
        discovery: Arc::new(discovery),
        transport: Arc::new(transport),
        local_device,
    };

    *get_instance().write() = Some(core);
    info!("Connected core initialized");
    Ok(())
}

pub fn start_discovery(callback: Box<dyn DiscoveryCallback>) -> Result<(), ConnectedError> {
    let instance = get_instance().read();
    let core = instance.as_ref().ok_or(ConnectedError::NotInitialized)?;

    if let Err(e) = core.discovery.announce() {
        warn!("Failed to re-announce: {}", e);
    }

    let (tx, mut rx) = mpsc::unbounded_channel::<DiscoveryEvent>();
    core.discovery
        .start_listening(tx)
        .map_err(|e| ConnectedError::Discovery(e.to_string()))?;

    let callback = Arc::new(callback);

    // Existing devices
    for device in core.discovery.get_discovered_devices() {
        callback.on_device_found(device.into());
    }

    get_runtime().spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                DiscoveryEvent::DeviceFound(d) => callback.on_device_found(d.into()),
                DiscoveryEvent::DeviceLost(id) => callback.on_device_lost(id),
                DiscoveryEvent::Error(msg) => callback.on_error(msg),
            }
        }
    });

    let (msg_tx, mut msg_rx) = mpsc::unbounded_channel();
    get_runtime().block_on(async {
        core.transport.start_server(msg_tx).await.ok();
    });

    get_runtime().spawn(async move {
        while let Some((addr, msg)) = msg_rx.recv().await {
            debug!("Message from {}: {:?}", addr, msg);
        }
    });

    Ok(())
}

pub fn stop_discovery() {
    if let Some(core) = get_instance().read().as_ref() {
        core.discovery.stop();
    }
}

pub fn get_local_device() -> Result<DiscoveredDevice, ConnectedError> {
    let instance = get_instance().read();
    let core = instance.as_ref().ok_or(ConnectedError::NotInitialized)?;
    Ok(core.local_device.clone().into())
}

pub fn send_ping(target_ip: String, target_port: u16) -> PingResult {
    let instance = get_instance().read();
    let core = match instance.as_ref() {
        Some(c) => c,
        None => {
            return PingResult {
                success: false,
                rtt_ms: 0,
                error_message: Some("Not initialized".into()),
            }
        }
    };

    let ip: IpAddr = match target_ip.parse() {
        Ok(ip) => ip,
        Err(_) => {
            return PingResult {
                success: false,
                rtt_ms: 0,
                error_message: Some("Invalid IP".into()),
            }
        }
    };

    match get_runtime().block_on(core.transport.send_ping(SocketAddr::new(ip, target_port))) {
        Ok(rtt) => PingResult {
            success: true,
            rtt_ms: rtt.as_millis() as u64,
            error_message: None,
        },
        Err(e) => PingResult {
            success: false,
            rtt_ms: 0,
            error_message: Some(e.to_string()),
        },
    }
}

pub fn send_file(
    target_ip: String,
    target_port: u16,
    file_path: String,
    callback: Box<dyn FileTransferCallback>,
) -> Result<(), ConnectedError> {
    let instance = get_instance().read();
    let core = instance.as_ref().ok_or(ConnectedError::NotInitialized)?;

    let path = PathBuf::from(&file_path);
    if !path.exists() {
        return Err(ConnectedError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "File not found",
        )));
    }

    let ip: IpAddr = target_ip
        .parse()
        .map_err(|_| ConnectedError::InitializationError("Invalid IP".into()))?;
    let target_addr = SocketAddr::new(ip, target_port);
    let transport = core.transport.clone();
    let callback = Arc::new(callback);

    get_runtime().spawn(async move {
        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
        let cb = callback.clone();

        tokio::spawn(async move {
            while let Some(progress) = progress_rx.recv().await {
                match progress {
                    TransferProgress::Starting {
                        filename,
                        total_size,
                    } => cb.on_transfer_starting(filename, total_size),
                    TransferProgress::Progress {
                        bytes_transferred,
                        total_size,
                    } => cb.on_transfer_progress(bytes_transferred, total_size),
                    TransferProgress::Completed {
                        filename,
                        total_size,
                    } => cb.on_transfer_completed(filename, total_size),
                    TransferProgress::Failed { error } => cb.on_transfer_failed(error),
                    TransferProgress::Cancelled => cb.on_transfer_cancelled(),
                }
            }
        });

        match transport.connect(target_addr).await {
            Ok(connection) => {
                let file_transfer = FileTransfer::new(connection);
                if let Err(e) = file_transfer.send_file(&path, Some(progress_tx)).await {
                    callback.on_transfer_failed(format!("Send failed: {}", e));
                }
            }
            Err(e) => callback.on_transfer_failed(format!("Connection failed: {}", e)),
        }
    });

    Ok(())
}

pub fn send_clipboard(
    target_ip: String,
    target_port: u16,
    text: String,
    callback: Box<dyn ClipboardCallback>,
) -> Result<(), ConnectedError> {
    let instance = get_instance().read();
    let core = instance.as_ref().ok_or(ConnectedError::NotInitialized)?;

    let ip: IpAddr = target_ip
        .parse()
        .map_err(|_| ConnectedError::InitializationError("Invalid IP".into()))?;
    let target_addr = SocketAddr::new(ip, target_port);
    let transport = core.transport.clone();
    let callback = Arc::new(callback);

    get_runtime().spawn(async move {
        match transport.connect(target_addr).await {
            Ok(connection) => {
                if let Ok((mut send, mut recv)) = connection.open_bi().await {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let msg = FileTransferMessage::ClipboardText { text: text.clone() };
                    let msg_data = serde_json::to_vec(&msg).unwrap();
                    let len_bytes = (msg_data.len() as u32).to_be_bytes();

                    if let Err(e) = send.write_all(&len_bytes).await {
                        callback.on_clipboard_sent(false, Some(e.to_string()));
                        return;
                    }
                    if let Err(e) = send.write_all(&msg_data).await {
                        callback.on_clipboard_sent(false, Some(e.to_string()));
                        return;
                    }
                    let _ = send.finish();

                    let mut len_buf = [0u8; 4];
                    if recv.read_exact(&mut len_buf).await.is_ok() {
                        let msg_len = u32::from_be_bytes(len_buf) as usize;
                        let mut msg_buf = vec![0u8; msg_len];
                        if recv.read_exact(&mut msg_buf).await.is_ok() {
                            callback.on_clipboard_sent(true, None);
                            return;
                        }
                    }
                    callback.on_clipboard_sent(true, None);
                } else {
                    callback.on_clipboard_sent(false, Some("Failed to open stream".into()));
                }
            }
            Err(e) => callback.on_clipboard_sent(false, Some(e.to_string())),
        }
    });
    Ok(())
}

pub fn register_clipboard_receiver(callback: Box<dyn ClipboardCallback>) {
    *get_clipboard_callback().write() = Some(Arc::from(callback));
}

pub fn handle_clipboard_message(text: String, from_addr: String) {
    if let Some(cb) = get_clipboard_callback().read().as_ref() {
        cb.on_clipboard_received(text, from_addr);
    }
}

pub fn start_file_receiver(
    save_dir: String,
    callback: Box<dyn FileTransferCallback>,
) -> Result<(), ConnectedError> {
    let instance = get_instance().read();
    let core = instance.as_ref().ok_or(ConnectedError::NotInitialized)?;
    let save_path = PathBuf::from(&save_dir);
    let local_device = core.local_device.clone();
    let callback: Arc<dyn FileTransferCallback> = Arc::from(callback);
    let runtime = get_runtime();

    runtime.block_on(async {
        tokio::fs::create_dir_all(&save_path)
            .await
            .map_err(|e| ConnectedError::Io(e))
    })?;

    let local_ip: IpAddr = local_device.ip.parse().map_err(|_| {
        ConnectedError::InitializationError("Invalid local IP stored in device".into())
    })?;
    let file_port = local_device.port + 1;
    let bind_addr = SocketAddr::new(local_ip, file_port);

    runtime.spawn(async move {
        use quinn::Endpoint;
        use tokio::io::AsyncReadExt;

        let server_config =
            crate::transport::QuicTransport::create_server_config().map_err(|e| {
                ConnectedError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.to_string(),
                ))
            })?;
        let endpoint =
            Endpoint::server(server_config, bind_addr).map_err(|e| ConnectedError::Io(e))?;

        info!("File receiver listening on {}", bind_addr);

        while let Some(incoming) = endpoint.accept().await {
            let save_path = save_path.clone();
            let cb = callback.clone();
            tokio::spawn(async move {
                if let Ok(connection) = incoming.await {
                    let remote_addr = connection.remote_address().to_string();
                    if let Ok((mut send, mut recv)) = connection.accept_bi().await {
                        let mut len_buf = [0u8; 4];
                        if recv.read_exact(&mut len_buf).await.is_err() {
                            return;
                        }
                        let msg_len = u32::from_be_bytes(len_buf) as usize;
                        let mut msg_buf = vec![0u8; msg_len];
                        if recv.read_exact(&mut msg_buf).await.is_err() {
                            return;
                        }

                        if let Ok(msg) = serde_json::from_slice::<FileTransferMessage>(&msg_buf) {
                            match msg {
                                FileTransferMessage::ClipboardText { text } => {
                                    handle_clipboard_message(text, remote_addr);
                                    let ack = FileTransferMessage::ClipboardAck;
                                    let ack_data = serde_json::to_vec(&ack).unwrap();
                                    let len_bytes = (ack_data.len() as u32).to_be_bytes();
                                    let _ = send.write_all(&len_bytes).await;
                                    let _ = send.write_all(&ack_data).await;
                                    let _ = send.finish();
                                }
                                FileTransferMessage::SendRequest { .. } => {
                                    handle_file_transfer_with_request(
                                        &msg_buf, &mut send, &mut recv, &save_path, cb,
                                    )
                                    .await;
                                }
                                _ => {}
                            }
                        }
                    }
                }
            });
        }
        Ok::<(), ConnectedError>(())
    });
    Ok(())
}

async fn handle_file_transfer_with_request(
    first_msg: &[u8],
    send: &mut quinn::SendStream,
    recv: &mut quinn::RecvStream,
    save_path: &PathBuf,
    callback: Arc<dyn FileTransferCallback>,
) {
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;

    let request: FileTransferMessage = match serde_json::from_slice(first_msg) {
        Ok(msg) => msg,
        Err(e) => {
            callback.on_transfer_failed(format!("Invalid request: {}", e));
            return;
        }
    };

    if let FileTransferMessage::SendRequest { filename, size, .. } = request {
        callback.on_transfer_starting(filename.clone(), size);
        let accept = FileTransferMessage::Accept;
        let accept_data = serde_json::to_vec(&accept).unwrap();
        let len_bytes = (accept_data.len() as u32).to_be_bytes();

        if send.write_all(&len_bytes).await.is_err() || send.write_all(&accept_data).await.is_err()
        {
            callback.on_transfer_failed("Failed to send accept".to_string());
            return;
        }

        let file_path = save_path.join(&filename);
        let mut file = match tokio::fs::File::create(&file_path).await {
            Ok(f) => f,
            Err(e) => {
                callback.on_transfer_failed(format!("Failed to create file: {}", e));
                return;
            }
        };

        let mut bytes_received: u64 = 0;
        loop {
            let mut len_buf = [0u8; 4];
            if recv.read_exact(&mut len_buf).await.is_err() {
                break;
            }
            let msg_len = u32::from_be_bytes(len_buf) as usize;
            let mut msg_buf = vec![0u8; msg_len];
            if recv.read_exact(&mut msg_buf).await.is_err() {
                break;
            }

            let msg: FileTransferMessage = match serde_json::from_slice(&msg_buf) {
                Ok(m) => m,
                Err(_) => break,
            };

            match msg {
                FileTransferMessage::Chunk { data, .. } => {
                    if file.write_all(&data).await.is_err() {
                        callback.on_transfer_failed("Write failed".to_string());
                        return;
                    }
                    bytes_received += data.len() as u64;
                    callback.on_transfer_progress(bytes_received, size);
                }
                FileTransferMessage::Complete { .. } => {
                    file.flush().await.ok();
                    callback.on_transfer_completed(filename.clone(), size);
                    let ack = FileTransferMessage::Ack;
                    let ack_data = serde_json::to_vec(&ack).unwrap();
                    let len_bytes = (ack_data.len() as u32).to_be_bytes();
                    let _ = send.write_all(&len_bytes).await;
                    let _ = send.write_all(&ack_data).await;
                    let _ = send.finish();
                    return;
                }
                FileTransferMessage::Cancel => {
                    callback.on_transfer_cancelled();
                    let _ = tokio::fs::remove_file(&file_path).await;
                    return;
                }
                _ => {}
            }
        }
    }
}

pub fn shutdown() {
    if let Some(core) = get_instance().write().take() {
        core.discovery.shutdown();
        get_runtime().block_on(async { core.transport.shutdown().await });
    }
}

fn get_local_ip() -> Option<IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    socket.local_addr().ok().map(|a| a.ip())
}
