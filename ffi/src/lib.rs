use connected_core::{ConnectedClient, ConnectedError, ConnectedEvent, Device, DeviceType};
use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use tokio::runtime::Runtime;
use tracing::{error, info};

#[cfg(target_os = "android")]
static ANDROID_LOGGER_INIT: OnceLock<()> = OnceLock::new();

#[cfg(target_os = "android")]
fn init_android_logging() {
    ANDROID_LOGGER_INIT.get_or_init(|| {
        android_logger::init_once(
            android_logger::Config::default()
                .with_max_level(log::LevelFilter::Debug)
                .with_tag("connected_core"),
        );
        tracing_log::LogTracer::init().ok();
    });
}

#[cfg(not(target_os = "android"))]
fn init_android_logging() {}

// ============================================================================
// Global State (The "Singleton" for FFI)
// ============================================================================

static RUNTIME: OnceLock<Runtime> = OnceLock::new();
static INSTANCE: OnceLock<RwLock<Option<Arc<ConnectedClient>>>> = OnceLock::new();

// Global Callbacks
static DISCOVERY_CALLBACK: RwLock<Option<Box<dyn DiscoveryCallback>>> = RwLock::new(None);
static CLIPBOARD_CALLBACK: RwLock<Option<Box<dyn ClipboardCallback>>> = RwLock::new(None);
static TRANSFER_CALLBACK: RwLock<Option<Box<dyn FileTransferCallback>>> = RwLock::new(None);
static PAIRING_CALLBACK: RwLock<Option<Box<dyn PairingCallback>>> = RwLock::new(None);
static UNPAIR_CALLBACK: RwLock<Option<Box<dyn UnpairCallback>>> = RwLock::new(None);

fn get_runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create Tokio runtime")
    })
}

fn get_client() -> Result<Arc<ConnectedClient>, ConnectedFfiError> {
    let lock = INSTANCE.get_or_init(|| RwLock::new(None));
    let read = lock.read();
    read.clone().ok_or(ConnectedFfiError::NotInitialized)
}

// ============================================================================
// UniFFI Types
// ============================================================================

#[derive(Debug, Clone, uniffi::Record)]
pub struct DiscoveredDevice {
    pub id: String,
    pub name: String,
    pub ip: String,
    pub port: u16,
    pub device_type: String,
}

impl From<Device> for DiscoveredDevice {
    fn from(d: Device) -> Self {
        Self {
            id: d.id,
            name: d.name,
            ip: d.ip.to_string(),
            port: d.port,
            device_type: d.device_type.as_str().to_string(),
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct TrustedPeer {
    pub fingerprint: String,
    pub name: String,
    pub device_id: String,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum ConnectedFfiError {
    #[error("Initialization error: {msg}")]
    InitializationError { msg: String },
    #[error("Discovery error: {msg}")]
    DiscoveryError { msg: String },
    #[error("Connection error: {msg}")]
    ConnectionError { msg: String },
    #[error("Not initialized")]
    NotInitialized,
    #[error("Invalid argument: {msg}")]
    InvalidArgument { msg: String },
}

impl From<ConnectedError> for ConnectedFfiError {
    fn from(err: ConnectedError) -> Self {
        ConnectedFfiError::ConnectionError {
            msg: err.to_string(),
        }
    }
}

// ============================================================================
// Callbacks
// ============================================================================

#[uniffi::export(callback_interface)]
pub trait DiscoveryCallback: Send + Sync {
    fn on_device_found(&self, device: DiscoveredDevice);
    fn on_device_lost(&self, device_id: String);
    fn on_error(&self, error_msg: String);
}

#[uniffi::export(callback_interface)]
pub trait FileTransferCallback: Send + Sync {
    fn on_transfer_request(
        &self,
        transfer_id: String,
        filename: String,
        file_size: u64,
        from_device: String,
    );
    fn on_transfer_starting(&self, filename: String, total_size: u64);
    fn on_transfer_progress(&self, bytes_transferred: u64, total_size: u64);
    fn on_transfer_completed(&self, filename: String, total_size: u64);
    fn on_transfer_failed(&self, error_msg: String);
    fn on_transfer_cancelled(&self);
}

#[uniffi::export(callback_interface)]
pub trait ClipboardCallback: Send + Sync {
    fn on_clipboard_received(&self, text: String, from_device: String);
    fn on_clipboard_sent(&self, success: bool, error_msg: Option<String>);
}

#[uniffi::export(callback_interface)]
pub trait PairingCallback: Send + Sync {
    fn on_pairing_request(&self, device_name: String, fingerprint: String, device_id: String);
}

#[uniffi::export(callback_interface)]
pub trait UnpairCallback: Send + Sync {
    fn on_device_unpaired(&self, device_id: String, device_name: String, reason: String);
}

// ============================================================================
// Exported Functions
// ============================================================================

#[uniffi::export]
pub fn initialize(
    device_name: String,
    device_type: String,
    bind_port: u16,
    storage_path: String,
) -> Result<(), ConnectedFfiError> {
    init_android_logging();

    let runtime = get_runtime();

    // Parse device type
    let dtype = DeviceType::from_str(&device_type); // assuming helper exists or we implement logic
                                                    // Actually DeviceType::from_str is available in core::device

    let path = if storage_path.is_empty() {
        None
    } else {
        Some(PathBuf::from(storage_path))
    };

    let client = runtime
        .block_on(async { ConnectedClient::new(device_name, dtype, bind_port, path).await })?;

    spawn_event_listener(client.clone(), runtime);

    let lock = INSTANCE.get_or_init(|| RwLock::new(None));
    *lock.write() = Some(client);

    Ok(())
}

#[uniffi::export]
pub fn initialize_with_ip(
    device_name: String,
    device_type: String,
    bind_port: u16,
    ip_address: String,
    storage_path: String,
) -> Result<(), ConnectedFfiError> {
    init_android_logging();
    let runtime = get_runtime();
    let dtype = DeviceType::from_str(&device_type);
    let ip: std::net::IpAddr =
        ip_address
            .parse()
            .map_err(|_| ConnectedFfiError::InvalidArgument {
                msg: "Invalid IP".into(),
            })?;

    let path = if storage_path.is_empty() {
        None
    } else {
        Some(PathBuf::from(storage_path))
    };

    let client = runtime.block_on(async {
        ConnectedClient::new_with_ip(device_name, dtype, bind_port, ip, path).await
    })?;

    spawn_event_listener(client.clone(), runtime);

    let lock = INSTANCE.get_or_init(|| RwLock::new(None));
    *lock.write() = Some(client);
    Ok(())
}

static SHUTDOWN_FLAG: OnceLock<Arc<std::sync::atomic::AtomicBool>> = OnceLock::new();

fn get_shutdown_flag() -> &'static Arc<std::sync::atomic::AtomicBool> {
    SHUTDOWN_FLAG.get_or_init(|| Arc::new(std::sync::atomic::AtomicBool::new(false)))
}

fn spawn_event_listener(client: Arc<ConnectedClient>, runtime: &Runtime) {
    let mut rx = client.subscribe();
    let shutdown_flag = get_shutdown_flag().clone();
    runtime.spawn(async move {
        while !shutdown_flag.load(std::sync::atomic::Ordering::SeqCst) {
            match rx.recv().await {
                Ok(event) => match event {
                    ConnectedEvent::DeviceFound(d) => {
                        if let Some(cb) = DISCOVERY_CALLBACK.read().as_ref() {
                            cb.on_device_found(d.into());
                        }
                    }
                    ConnectedEvent::DeviceLost(id) => {
                        if let Some(cb) = DISCOVERY_CALLBACK.read().as_ref() {
                            cb.on_device_lost(id);
                        }
                    }
                    ConnectedEvent::ClipboardReceived {
                        content,
                        from_device,
                    } => {
                        if let Some(cb) = CLIPBOARD_CALLBACK.read().as_ref() {
                            cb.on_clipboard_received(content, from_device);
                        }
                    }
                    ConnectedEvent::TransferRequest {
                        id,
                        filename,
                        size,
                        from_device,
                        from_fingerprint: _,
                    } => {
                        if let Some(cb) = TRANSFER_CALLBACK.read().as_ref() {
                            cb.on_transfer_request(id, filename, size, from_device);
                        }
                    }
                    ConnectedEvent::TransferStarting {
                        filename,
                        total_size,
                        ..
                    } => {
                        if let Some(cb) = TRANSFER_CALLBACK.read().as_ref() {
                            cb.on_transfer_starting(filename, total_size);
                        }
                    }
                    ConnectedEvent::TransferProgress {
                        bytes_transferred,
                        total_size,
                        ..
                    } => {
                        if let Some(cb) = TRANSFER_CALLBACK.read().as_ref() {
                            cb.on_transfer_progress(bytes_transferred, total_size);
                        }
                    }
                    ConnectedEvent::TransferCompleted { filename, .. } => {
                        if let Some(cb) = TRANSFER_CALLBACK.read().as_ref() {
                            cb.on_transfer_completed(filename, 0);
                        }
                    }
                    ConnectedEvent::TransferFailed { error, .. } => {
                        if let Some(cb) = TRANSFER_CALLBACK.read().as_ref() {
                            cb.on_transfer_failed(error);
                        }
                    }
                    ConnectedEvent::PairingRequest {
                        fingerprint,
                        device_name,
                        device_id,
                    } => {
                        if let Some(cb) = PAIRING_CALLBACK.read().as_ref() {
                            cb.on_pairing_request(device_name, fingerprint, device_id);
                        }
                    }
                    ConnectedEvent::DeviceUnpaired {
                        device_id,
                        device_name,
                        reason,
                    } => {
                        use connected_core::transport::UnpairReason;
                        let reason_str = match reason {
                            UnpairReason::Unpaired => "unpaired",
                            UnpairReason::Forgotten => "forgotten",
                            UnpairReason::Blocked => "blocked",
                        };
                        if let Some(cb) = UNPAIR_CALLBACK.read().as_ref() {
                            cb.on_device_unpaired(device_id, device_name, reason_str.to_string());
                        }
                    }
                    ConnectedEvent::Error(msg) => {
                        error!("Core error: {}", msg);
                    }
                    _ => {}
                },
                Err(_) => {
                    // Channel closed or lagged, check shutdown flag
                    if shutdown_flag.load(std::sync::atomic::Ordering::SeqCst) {
                        break;
                    }
                }
            }
        }
        info!("Event listener stopped");
    });
}

#[uniffi::export]
pub fn start_discovery(callback: Box<dyn DiscoveryCallback>) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    *DISCOVERY_CALLBACK.write() = Some(callback);
    // Client auto-discovers, but we can trigger it or just ensure we send existing devices
    // The client doesn't expose `announce` publicly again but it does it on init.
    // We can iterate existing devices and callback.
    let devices = client.get_discovered_devices();
    if let Some(cb) = DISCOVERY_CALLBACK.read().as_ref() {
        for d in devices {
            cb.on_device_found(d.into());
        }
    }
    Ok(())
}

#[uniffi::export]
pub fn stop_discovery() {
    *DISCOVERY_CALLBACK.write() = None;
}

#[uniffi::export]
pub fn get_discovered_devices() -> Result<Vec<DiscoveredDevice>, ConnectedFfiError> {
    let client = get_client()?;
    Ok(client
        .get_discovered_devices()
        .into_iter()
        .map(Into::into)
        .collect())
}

#[uniffi::export]
pub fn clear_discovered_devices() -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    client.clear_discovered_devices();
    Ok(())
}

#[uniffi::export]
pub fn get_local_device() -> Result<DiscoveredDevice, ConnectedFfiError> {
    let client = get_client()?;
    Ok(client.local_device().clone().into())
}

#[uniffi::export]
pub fn get_local_fingerprint() -> Result<String, ConnectedFfiError> {
    let client = get_client()?;
    Ok(client.get_fingerprint())
}

#[uniffi::export]
pub fn send_file(
    target_ip: String,
    target_port: u16,
    file_path: String,
) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    let ip: std::net::IpAddr =
        target_ip
            .parse()
            .map_err(|_| ConnectedFfiError::InvalidArgument {
                msg: "Invalid IP".into(),
            })?;

    let path = PathBuf::from(file_path);
    get_runtime().spawn(async move {
        let _ = client.send_file(ip, target_port, path).await;
    });

    Ok(())
}

#[uniffi::export]
pub fn register_transfer_callback(callback: Box<dyn FileTransferCallback>) {
    *TRANSFER_CALLBACK.write() = Some(callback);
}

#[uniffi::export]
pub fn send_clipboard(
    target_ip: String,
    target_port: u16,
    text: String,
    callback: Box<dyn ClipboardCallback>,
) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    let ip: std::net::IpAddr =
        target_ip
            .parse()
            .map_err(|_| ConnectedFfiError::InvalidArgument {
                msg: "Invalid IP".into(),
            })?;

    get_runtime().spawn(async move {
        match client.send_clipboard(ip, target_port, text).await {
            Ok(_) => callback.on_clipboard_sent(true, None),
            Err(e) => callback.on_clipboard_sent(false, Some(e.to_string())),
        }
    });
    Ok(())
}

#[uniffi::export]
pub fn register_clipboard_receiver(callback: Box<dyn ClipboardCallback>) {
    *CLIPBOARD_CALLBACK.write() = Some(callback);
}

#[uniffi::export]
pub fn register_pairing_callback(callback: Box<dyn PairingCallback>) {
    *PAIRING_CALLBACK.write() = Some(callback);
}

#[uniffi::export]
pub fn register_unpair_callback(callback: Box<dyn UnpairCallback>) {
    *UNPAIR_CALLBACK.write() = Some(callback);
}

#[uniffi::export]
pub fn set_pairing_mode(enabled: bool) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    client.set_pairing_mode(enabled);
    Ok(())
}

#[uniffi::export]
pub fn pair_device(target_ip: String, target_port: u16) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    let ip: std::net::IpAddr =
        target_ip
            .parse()
            .map_err(|_| ConnectedFfiError::InvalidArgument {
                msg: "Invalid IP".into(),
            })?;

    get_runtime().spawn(async move {
        // send_handshake now automatically enables pairing mode with timeout
        match client.send_handshake(ip, target_port).await {
            Ok(()) => {
                info!("Pairing completed successfully with {}:{}", ip, target_port);
            }
            Err(e) => {
                error!("Failed to pair with {}:{}: {}", ip, target_port, e);
            }
        }
    });

    Ok(())
}

#[uniffi::export]
pub fn send_trust_confirmation(
    target_ip: String,
    target_port: u16,
) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    let ip: std::net::IpAddr =
        target_ip
            .parse()
            .map_err(|_| ConnectedFfiError::InvalidArgument {
                msg: "Invalid IP".into(),
            })?;

    get_runtime().spawn(async move {
        if let Err(e) = client.send_trust_confirmation(ip, target_port).await {
            error!(
                "Failed to send trust confirmation to {}:{}: {}",
                ip, target_port, e
            );
        }
    });

    Ok(())
}

#[uniffi::export]
pub fn send_unpair_notification(
    target_ip: String,
    target_port: u16,
    reason: String,
) -> Result<(), ConnectedFfiError> {
    use connected_core::transport::UnpairReason;

    let client = get_client()?;
    let ip: std::net::IpAddr =
        target_ip
            .parse()
            .map_err(|_| ConnectedFfiError::InvalidArgument {
                msg: "Invalid IP".into(),
            })?;

    let unpair_reason = match reason.as_str() {
        "forgotten" => UnpairReason::Forgotten,
        "blocked" => UnpairReason::Blocked,
        _ => UnpairReason::Unpaired,
    };

    get_runtime().spawn(async move {
        if let Err(e) = client
            .send_unpair_notification(ip, target_port, unpair_reason)
            .await
        {
            error!(
                "Failed to send unpair notification to {}:{}: {}",
                ip, target_port, e
            );
        }
    });

    Ok(())
}

#[uniffi::export]
pub fn trust_device(
    fingerprint: String,
    device_id: String,
    name: String,
) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    client
        .trust_device(fingerprint, Some(device_id), name)
        .map_err(Into::into)
}

#[uniffi::export]
pub fn unpair_device(fingerprint: String) -> Result<(), ConnectedFfiError> {
    // Unpair = disconnect but keep trust intact (can reconnect anytime without re-pairing)
    let client = get_client()?;
    client.unpair_device(&fingerprint).map_err(Into::into)
}

#[uniffi::export]
pub fn unpair_device_by_id(device_id: String) -> Result<(), ConnectedFfiError> {
    // Unpair = disconnect but keep trust intact (can reconnect anytime without re-pairing)
    let client = get_client()?;
    client.unpair_device_by_id(&device_id).map_err(Into::into)
}

#[uniffi::export]
pub fn forget_device(fingerprint: String) -> Result<(), ConnectedFfiError> {
    // Forget = completely remove trust (must re-pair to connect again)
    let client = get_client()?;
    client.forget_device(&fingerprint).map_err(Into::into)
}

#[uniffi::export]
pub fn forget_device_by_id(device_id: String) -> Result<(), ConnectedFfiError> {
    // Forget = completely remove trust (must re-pair to connect again)
    let client = get_client()?;
    client.forget_device_by_id(&device_id).map_err(Into::into)
}

#[uniffi::export]
pub fn is_device_forgotten(fingerprint: String) -> bool {
    if let Ok(client) = get_client() {
        client.is_device_forgotten(&fingerprint)
    } else {
        false
    }
}

#[uniffi::export]
pub fn is_device_trusted(device_id: String) -> bool {
    if let Ok(client) = get_client() {
        client.is_device_trusted(&device_id)
    } else {
        false
    }
}

#[uniffi::export]
pub fn block_device(fingerprint: String) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    client.block_device(fingerprint).map_err(Into::into)
}

#[uniffi::export]
pub fn block_device_by_id(device_id: String) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    // Look up fingerprint from device_id in trusted/known peers
    let peers = client.get_all_known_peers();
    let fingerprint = peers
        .iter()
        .find(|p| p.device_id.as_deref() == Some(&device_id))
        .map(|p| p.fingerprint.clone());

    if let Some(fp) = fingerprint {
        client.block_device(fp).map_err(Into::into)
    } else {
        Err(ConnectedFfiError::InvalidArgument {
            msg: format!("Device {} not found in known peers", device_id),
        })
    }
}

#[uniffi::export]
pub fn accept_file_transfer(transfer_id: String) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    client
        .accept_file_transfer(&transfer_id)
        .map_err(Into::into)
}

#[uniffi::export]
pub fn reject_file_transfer(transfer_id: String) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    client
        .reject_file_transfer(&transfer_id)
        .map_err(Into::into)
}

#[uniffi::export]
pub fn set_auto_accept_files(enabled: bool) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    client.set_auto_accept_files(enabled);
    Ok(())
}

#[uniffi::export]
pub fn is_auto_accept_files() -> Result<bool, ConnectedFfiError> {
    let client = get_client()?;
    Ok(client.is_auto_accept_files())
}

#[uniffi::export]
pub fn shutdown() {
    info!("Shutting down connected FFI...");

    // Signal event listener to stop
    get_shutdown_flag().store(true, std::sync::atomic::Ordering::SeqCst);

    // Clear callbacks first
    *DISCOVERY_CALLBACK.write() = None;
    *CLIPBOARD_CALLBACK.write() = None;
    *TRANSFER_CALLBACK.write() = None;
    *PAIRING_CALLBACK.write() = None;
    *UNPAIR_CALLBACK.write() = None;

    // Get and shutdown the client properly
    let client = {
        let mut lock = INSTANCE.get_or_init(|| RwLock::new(None)).write();
        lock.take()
    };

    if let Some(c) = client {
        // Call the proper shutdown method
        get_runtime().block_on(async {
            c.shutdown().await;
        });
        drop(c);
    }

    // Reset shutdown flag for potential re-initialization
    get_shutdown_flag().store(false, std::sync::atomic::Ordering::SeqCst);

    info!("Connected FFI shutdown complete");
}

// ============================================================================
// Filesystem Support
// ============================================================================

#[derive(Debug, Clone, uniffi::Enum)]
pub enum FfiFsEntryType {
    File,
    Directory,
    Symlink,
    Unknown,
}

impl From<connected_core::filesystem::FsEntryType> for FfiFsEntryType {
    fn from(t: connected_core::filesystem::FsEntryType) -> Self {
        match t {
            connected_core::filesystem::FsEntryType::File => FfiFsEntryType::File,
            connected_core::filesystem::FsEntryType::Directory => FfiFsEntryType::Directory,
            connected_core::filesystem::FsEntryType::Symlink => FfiFsEntryType::Symlink,
            connected_core::filesystem::FsEntryType::Unknown => FfiFsEntryType::Unknown,
        }
    }
}

impl From<FfiFsEntryType> for connected_core::filesystem::FsEntryType {
    fn from(t: FfiFsEntryType) -> Self {
        match t {
            FfiFsEntryType::File => connected_core::filesystem::FsEntryType::File,
            FfiFsEntryType::Directory => connected_core::filesystem::FsEntryType::Directory,
            FfiFsEntryType::Symlink => connected_core::filesystem::FsEntryType::Symlink,
            FfiFsEntryType::Unknown => connected_core::filesystem::FsEntryType::Unknown,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FfiFsEntry {
    pub name: String,
    pub path: String,
    pub entry_type: FfiFsEntryType,
    pub size: u64,
    pub modified: Option<u64>,
}

impl From<connected_core::filesystem::FsEntry> for FfiFsEntry {
    fn from(e: connected_core::filesystem::FsEntry) -> Self {
        Self {
            name: e.name,
            path: e.path,
            entry_type: e.entry_type.into(),
            size: e.size,
            modified: e.modified,
        }
    }
}

impl From<FfiFsEntry> for connected_core::filesystem::FsEntry {
    fn from(e: FfiFsEntry) -> Self {
        Self {
            name: e.name,
            path: e.path,
            entry_type: e.entry_type.into(),
            size: e.size,
            modified: e.modified,
        }
    }
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FilesystemError {
    #[error("Filesystem error: {msg}")]
    Generic { msg: String },
}

#[uniffi::export(callback_interface)]
pub trait FilesystemProviderCallback: Send + Sync {
    fn list_dir(&self, path: String) -> Result<Vec<FfiFsEntry>, FilesystemError>;
    fn read_file(&self, path: String, offset: u64, size: u64) -> Result<Vec<u8>, FilesystemError>;
    fn write_file(&self, path: String, offset: u64, data: Vec<u8>) -> Result<u64, FilesystemError>;
    fn get_metadata(&self, path: String) -> Result<FfiFsEntry, FilesystemError>;
}

static FS_PROVIDER: RwLock<Option<Box<dyn FilesystemProviderCallback>>> = RwLock::new(None);

struct FfiFilesystemProviderBridge;
impl connected_core::filesystem::FilesystemProvider for FfiFilesystemProviderBridge {
    fn list_dir(
        &self,
        path: &str,
    ) -> connected_core::Result<Vec<connected_core::filesystem::FsEntry>> {
        if let Some(cb) = FS_PROVIDER.read().as_ref() {
            cb.list_dir(path.to_string())
                .map_err(|e| connected_core::ConnectedError::Filesystem(e.to_string()))
                .map(|v| v.into_iter().map(Into::into).collect())
        } else {
            Err(connected_core::ConnectedError::Filesystem(
                "No provider registered".to_string(),
            ))
        }
    }

    fn read_file(&self, path: &str, offset: u64, size: u64) -> connected_core::Result<Vec<u8>> {
        if let Some(cb) = FS_PROVIDER.read().as_ref() {
            cb.read_file(path.to_string(), offset, size)
                .map_err(|e| connected_core::ConnectedError::Filesystem(e.to_string()))
        } else {
            Err(connected_core::ConnectedError::Filesystem(
                "No provider registered".to_string(),
            ))
        }
    }

    fn write_file(&self, path: &str, offset: u64, data: &[u8]) -> connected_core::Result<u64> {
        if let Some(cb) = FS_PROVIDER.read().as_ref() {
            cb.write_file(path.to_string(), offset, data.to_vec())
                .map_err(|e| connected_core::ConnectedError::Filesystem(e.to_string()))
        } else {
            Err(connected_core::ConnectedError::Filesystem(
                "No provider registered".to_string(),
            ))
        }
    }

    fn get_metadata(
        &self,
        path: &str,
    ) -> connected_core::Result<connected_core::filesystem::FsEntry> {
        if let Some(cb) = FS_PROVIDER.read().as_ref() {
            cb.get_metadata(path.to_string())
                .map_err(|e| connected_core::ConnectedError::Filesystem(e.to_string()))
                .map(Into::into)
        } else {
            Err(connected_core::ConnectedError::Filesystem(
                "No provider registered".to_string(),
            ))
        }
    }
}

#[uniffi::export]
pub fn register_filesystem_provider(
    callback: Box<dyn FilesystemProviderCallback>,
) -> Result<(), ConnectedFfiError> {
    *FS_PROVIDER.write() = Some(callback);
    let client = get_client()?;
    client.register_filesystem_provider(Box::new(FfiFilesystemProviderBridge));
    Ok(())
}

#[uniffi::export]
pub fn request_list_dir(
    target_ip: String,
    target_port: u16,
    path: String,
) -> Result<Vec<FfiFsEntry>, ConnectedFfiError> {
    let client = get_client()?;
    let ip: std::net::IpAddr =
        target_ip
            .parse()
            .map_err(|_| ConnectedFfiError::InvalidArgument {
                msg: "Invalid IP".into(),
            })?;

    // Block on async call - Caller must run this in background thread!
    let entries =
        get_runtime().block_on(async { client.fs_list_dir(ip, target_port, path).await })?;

    Ok(entries.into_iter().map(Into::into).collect())
}

uniffi::setup_scaffolding!();
