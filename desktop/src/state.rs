use connected_core::facade::DiscoveredDevice;
use std::collections::HashMap;
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
}

impl From<DiscoveredDevice> for DeviceInfo {
    fn from(d: DiscoveredDevice) -> Self {
        Self {
            id: d.id,
            name: d.name,
            ip: d.ip,
            port: d.port,
            device_type: d.device_type,
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
pub enum ClipboardStatus {
    Idle,
    Syncing { device_name: String },
    Received { from: String, text: String },
    Sent { success: bool },
}

#[derive(Debug, Clone)]
pub struct Notification {
    pub id: u64,
    pub title: String,
    pub message: String,
    pub icon: &'static str,
    pub timestamp: std::time::Instant,
}

// ============================================================================
// Global Stores
// ============================================================================

static DEVICES: OnceLock<Arc<Mutex<HashMap<String, DeviceInfo>>>> = OnceLock::new();
static TRANSFER_STATUS: OnceLock<Arc<Mutex<TransferStatus>>> = OnceLock::new();
static CLIPBOARD_STATUS: OnceLock<Arc<Mutex<ClipboardStatus>>> = OnceLock::new();
static NOTIFICATIONS: OnceLock<Arc<Mutex<Vec<Notification>>>> = OnceLock::new();
static NOTIFICATION_COUNTER: OnceLock<Arc<Mutex<u64>>> = OnceLock::new();
static LAST_CLIPBOARD: OnceLock<Arc<Mutex<String>>> = OnceLock::new();

pub fn get_devices_store() -> &'static Arc<Mutex<HashMap<String, DeviceInfo>>> {
    DEVICES.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

pub fn get_transfer_status() -> &'static Arc<Mutex<TransferStatus>> {
    TRANSFER_STATUS.get_or_init(|| Arc::new(Mutex::new(TransferStatus::Idle)))
}

pub fn get_clipboard_status() -> &'static Arc<Mutex<ClipboardStatus>> {
    CLIPBOARD_STATUS.get_or_init(|| Arc::new(Mutex::new(ClipboardStatus::Idle)))
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
