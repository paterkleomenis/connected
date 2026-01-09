use connected_core::filesystem::FsEntry;
use connected_core::Device;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

// ============================================================================
// Data Models
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub ip: String,
    pub port: u16,
    pub device_type: String,
    pub is_trusted: bool,
    pub is_pending: bool,
}

impl From<Device> for DeviceInfo {
    fn from(d: Device) -> Self {
        Self {
            id: d.id,
            name: d.name,
            ip: d.ip.to_string(),
            port: d.port,
            device_type: d.device_type.as_str().to_string(),
            is_trusted: false,
            is_pending: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TransferStatus {
    Idle,
    Starting(String),
    InProgress { filename: String, percent: f32 },
    Completed(String),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    pub id: u64,
    pub title: String,
    pub message: String,
    pub icon: &'static str,
    pub timestamp: std::time::Instant,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PairingRequest {
    pub fingerprint: String,
    pub device_name: String,
    pub device_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileTransferRequest {
    pub id: String,
    pub filename: String,
    pub size: u64,
    pub from_device: String,
    pub from_fingerprint: String,
    pub timestamp: std::time::Instant,
}

// ============================================================================
// Global Stores
// ============================================================================

static DEVICES: OnceLock<Arc<Mutex<HashMap<String, DeviceInfo>>>> = OnceLock::new();
static TRANSFER_STATUS: OnceLock<Arc<Mutex<TransferStatus>>> = OnceLock::new();
static NOTIFICATIONS: OnceLock<Arc<Mutex<Vec<Notification>>>> = OnceLock::new();
static NOTIFICATION_COUNTER: OnceLock<Arc<Mutex<u64>>> = OnceLock::new();
static LAST_CLIPBOARD: OnceLock<Arc<Mutex<String>>> = OnceLock::new();
static LAST_REMOTE_UPDATE: OnceLock<Arc<Mutex<std::time::Instant>>> = OnceLock::new();
static PAIRING_REQUESTS: OnceLock<Arc<Mutex<Vec<PairingRequest>>>> = OnceLock::new();
static PENDING_PAIRINGS: OnceLock<Arc<Mutex<HashSet<String>>>> = OnceLock::new();
static FILE_TRANSFER_REQUESTS: OnceLock<Arc<Mutex<HashMap<String, FileTransferRequest>>>> =
    OnceLock::new();
static REMOTE_FILES: OnceLock<Arc<Mutex<Option<Vec<FsEntry>>>>> = OnceLock::new();
static REMOTE_PATH: OnceLock<Arc<Mutex<String>>> = OnceLock::new();
static REMOTE_FILES_UPDATE: OnceLock<Arc<Mutex<std::time::Instant>>> = OnceLock::new();

pub fn get_devices_store() -> &'static Arc<Mutex<HashMap<String, DeviceInfo>>> {
    DEVICES.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

pub fn get_transfer_status() -> &'static Arc<Mutex<TransferStatus>> {
    TRANSFER_STATUS.get_or_init(|| Arc::new(Mutex::new(TransferStatus::Idle)))
}

pub fn get_notifications() -> &'static Arc<Mutex<Vec<Notification>>> {
    NOTIFICATIONS.get_or_init(|| Arc::new(Mutex::new(Vec::new())))
}

pub fn get_notification_counter() -> &'static Arc<Mutex<u64>> {
    NOTIFICATION_COUNTER.get_or_init(|| Arc::new(Mutex::new(0)))
}

pub fn get_last_clipboard() -> &'static Arc<Mutex<String>> {
    LAST_CLIPBOARD.get_or_init(|| Arc::new(Mutex::new(String::new())))
}

pub fn get_last_remote_update() -> &'static Arc<Mutex<std::time::Instant>> {
    LAST_REMOTE_UPDATE.get_or_init(|| Arc::new(Mutex::new(std::time::Instant::now())))
}

pub fn get_pairing_requests() -> &'static Arc<Mutex<Vec<PairingRequest>>> {
    PAIRING_REQUESTS.get_or_init(|| Arc::new(Mutex::new(Vec::new())))
}

pub fn get_pending_pairings() -> &'static Arc<Mutex<HashSet<String>>> {
    PENDING_PAIRINGS.get_or_init(|| Arc::new(Mutex::new(HashSet::new())))
}

pub fn get_file_transfer_requests() -> &'static Arc<Mutex<HashMap<String, FileTransferRequest>>> {
    FILE_TRANSFER_REQUESTS.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

pub fn add_file_transfer_request(request: FileTransferRequest) {
    let mut requests = get_file_transfer_requests().lock().unwrap();
    requests.insert(request.id.clone(), request);
}

pub fn remove_file_transfer_request(id: &str) -> Option<FileTransferRequest> {
    let mut requests = get_file_transfer_requests().lock().unwrap();
    requests.remove(id)
}

pub fn add_notification(title: &str, message: &str, icon: &'static str) {
    let mut counter = get_notification_counter().lock().unwrap();
    *counter += 1;
    let id = *counter;
    drop(counter);

    let notification = Notification {
        id,
        title: title.to_string(),
        message: message.to_string(),
        icon,
        timestamp: std::time::Instant::now(),
    };

    let mut notifications = get_notifications().lock().unwrap();
    notifications.push(notification);
    // Keep only last 5 notifications
    if notifications.len() > 5 {
        notifications.remove(0);
    }
}

pub fn get_current_remote_files() -> &'static Arc<Mutex<Option<Vec<FsEntry>>>> {
    REMOTE_FILES.get_or_init(|| Arc::new(Mutex::new(None)))
}

pub fn get_current_remote_path() -> &'static Arc<Mutex<String>> {
    REMOTE_PATH.get_or_init(|| Arc::new(Mutex::new(String::from("/"))))
}

pub fn get_remote_files_update() -> &'static Arc<Mutex<std::time::Instant>> {
    REMOTE_FILES_UPDATE.get_or_init(|| Arc::new(Mutex::new(std::time::Instant::now())))
}
