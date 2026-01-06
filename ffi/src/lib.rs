use connected_core::facade;
use std::sync::OnceLock;
use tracing::info;

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
fn init_android_logging() {
    // No-op on non-Android platforms
}

// ============================================================================
// UniFFI Types and Errors
// ============================================================================

#[derive(Debug, Clone, uniffi::Record)]
pub struct DiscoveredDevice {
    pub id: String,
    pub name: String,
    pub ip: String,
    pub port: u16,
    pub device_type: String,
}

impl From<facade::DiscoveredDevice> for DiscoveredDevice {
    fn from(d: facade::DiscoveredDevice) -> Self {
        Self {
            id: d.id,
            name: d.name,
            ip: d.ip,
            port: d.port,
            device_type: d.device_type,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct PingResult {
    pub success: bool,
    pub rtt_ms: u64,
    pub error_message: Option<String>,
}

impl From<facade::PingResult> for PingResult {
    fn from(r: facade::PingResult) -> Self {
        Self {
            success: r.success,
            rtt_ms: r.rtt_ms,
            error_message: r.error_message,
        }
    }
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

impl From<connected_core::ConnectedError> for ConnectedFfiError {
    fn from(err: connected_core::ConnectedError) -> Self {
        use connected_core::ConnectedError;
        match err {
            ConnectedError::Discovery(msg) => ConnectedFfiError::DiscoveryError { msg },
            ConnectedError::Mdns(e) => ConnectedFfiError::DiscoveryError { msg: e.to_string() },
            ConnectedError::Io(e) => ConnectedFfiError::ConnectionError { msg: e.to_string() },
            ConnectedError::Timeout(msg) => ConnectedFfiError::ConnectionError { msg },
            ConnectedError::PingFailed(msg) => ConnectedFfiError::ConnectionError { msg },
            ConnectedError::NotInitialized => ConnectedFfiError::NotInitialized,
            _ => ConnectedFfiError::ConnectionError {
                msg: err.to_string(),
            },
        }
    }
}

// ============================================================================
// UniFFI Callbacks
// ============================================================================

#[uniffi::export(callback_interface)]
pub trait DiscoveryCallback: Send + Sync {
    fn on_device_found(&self, device: DiscoveredDevice);
    fn on_device_lost(&self, device_id: String);
    fn on_error(&self, error_msg: String);
}

struct DiscoveryCallbackWrapper(Box<dyn DiscoveryCallback>);

impl facade::DiscoveryCallback for DiscoveryCallbackWrapper {
    fn on_device_found(&self, device: facade::DiscoveredDevice) {
        self.0.on_device_found(device.into());
    }
    fn on_device_lost(&self, device_id: String) {
        self.0.on_device_lost(device_id);
    }
    fn on_error(&self, error_msg: String) {
        self.0.on_error(error_msg);
    }
}

#[uniffi::export(callback_interface)]
pub trait FileTransferCallback: Send + Sync {
    fn on_transfer_starting(&self, filename: String, total_size: u64);
    fn on_transfer_progress(&self, bytes_transferred: u64, total_size: u64);
    fn on_transfer_completed(&self, filename: String, total_size: u64);
    fn on_transfer_failed(&self, error_msg: String);
    fn on_transfer_cancelled(&self);
}

struct FileTransferCallbackWrapper(Box<dyn FileTransferCallback>);

impl facade::FileTransferCallback for FileTransferCallbackWrapper {
    fn on_transfer_starting(&self, filename: String, total_size: u64) {
        self.0.on_transfer_starting(filename, total_size);
    }
    fn on_transfer_progress(&self, bytes_transferred: u64, total_size: u64) {
        self.0.on_transfer_progress(bytes_transferred, total_size);
    }
    fn on_transfer_completed(&self, filename: String, total_size: u64) {
        self.0.on_transfer_completed(filename, total_size);
    }
    fn on_transfer_failed(&self, error_msg: String) {
        self.0.on_transfer_failed(error_msg);
    }
    fn on_transfer_cancelled(&self) {
        self.0.on_transfer_cancelled();
    }
}

#[uniffi::export(callback_interface)]
pub trait ClipboardCallback: Send + Sync {
    fn on_clipboard_received(&self, text: String, from_device: String);
    fn on_clipboard_sent(&self, success: bool, error_msg: Option<String>);
}

struct ClipboardCallbackWrapper(Box<dyn ClipboardCallback>);

impl facade::ClipboardCallback for ClipboardCallbackWrapper {
    fn on_clipboard_received(&self, text: String, from_device: String) {
        self.0.on_clipboard_received(text, from_device);
    }
    fn on_clipboard_sent(&self, success: bool, error_msg: Option<String>) {
        self.0.on_clipboard_sent(success, error_msg);
    }
}

// ============================================================================
// Exported Functions
// ============================================================================

#[uniffi::export]
pub fn initialize(
    device_name: String,
    device_type: String,
    bind_port: u16,
) -> Result<(), ConnectedFfiError> {
    init_android_logging();
    facade::initialize(device_name, device_type, bind_port).map_err(Into::into)
}

#[uniffi::export]
pub fn initialize_with_ip(
    device_name: String,
    device_type: String,
    bind_port: u16,
    ip_address: String,
) -> Result<(), ConnectedFfiError> {
    init_android_logging();
    facade::initialize_with_ip(device_name, device_type, bind_port, ip_address).map_err(Into::into)
}

#[uniffi::export]
pub fn start_discovery(callback: Box<dyn DiscoveryCallback>) -> Result<(), ConnectedFfiError> {
    let wrapper = Box::new(DiscoveryCallbackWrapper(callback));
    facade::start_discovery(wrapper).map_err(Into::into)
}

#[uniffi::export]
pub fn stop_discovery() {
    facade::stop_discovery();
}

#[uniffi::export]
pub fn get_local_device() -> Result<DiscoveredDevice, ConnectedFfiError> {
    facade::get_local_device()
        .map(Into::into)
        .map_err(Into::into)
}

#[uniffi::export]
pub fn send_ping(target_ip: String, target_port: u16) -> PingResult {
    facade::send_ping(target_ip, target_port).into()
}

#[uniffi::export]
pub fn send_file(
    target_ip: String,
    target_port: u16,
    file_path: String,
    callback: Box<dyn FileTransferCallback>,
) -> Result<(), ConnectedFfiError> {
    let wrapper = Box::new(FileTransferCallbackWrapper(callback));
    facade::send_file(target_ip, target_port, file_path, wrapper).map_err(Into::into)
}

#[uniffi::export]
pub fn send_clipboard(
    target_ip: String,
    target_port: u16,
    text: String,
    callback: Box<dyn ClipboardCallback>,
) -> Result<(), ConnectedFfiError> {
    let wrapper = Box::new(ClipboardCallbackWrapper(callback));
    facade::send_clipboard(target_ip, target_port, text, wrapper).map_err(Into::into)
}

#[uniffi::export]
pub fn register_clipboard_receiver(callback: Box<dyn ClipboardCallback>) {
    let wrapper = Box::new(ClipboardCallbackWrapper(callback));
    facade::register_clipboard_receiver(wrapper);
}

#[uniffi::export]
pub fn start_file_receiver(
    save_dir: String,
    callback: Box<dyn FileTransferCallback>,
) -> Result<(), ConnectedFfiError> {
    let wrapper = Box::new(FileTransferCallbackWrapper(callback));
    facade::start_file_receiver(save_dir, wrapper).map_err(Into::into)
}

#[uniffi::export]
pub fn shutdown() {
    facade::shutdown();
}

uniffi::setup_scaffolding!();
