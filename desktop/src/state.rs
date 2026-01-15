use connected_core::filesystem::FsEntry;
use connected_core::telephony::{ActiveCall, CallLogEntry, Contact, Conversation, SmsMessage};
use connected_core::{Device, MediaState};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

// ============================================================================
// Persistent Settings
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub clipboard_sync_enabled: bool,
    pub media_enabled: bool,
    pub auto_sync_messages: bool,
    pub auto_sync_calls: bool,
    pub auto_sync_contacts: bool,
    pub notifications_enabled: bool,
    pub device_name: Option<String>,
    pub saved_devices: HashMap<String, SavedDeviceInfo>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            clipboard_sync_enabled: false,
            media_enabled: false,
            auto_sync_messages: false,
            auto_sync_calls: false,
            auto_sync_contacts: false,
            notifications_enabled: true,
            device_name: None,
            saved_devices: HashMap::new(),
        }
    }
}

static APP_SETTINGS: OnceLock<Arc<Mutex<AppSettings>>> = OnceLock::new();

fn get_settings_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("connected")
        .join("settings.json")
}

pub fn load_settings() -> AppSettings {
    let path = get_settings_path();
    if path.exists() {
        if let Ok(contents) = fs::read_to_string(&path) {
            if let Ok(settings) = serde_json::from_str(&contents) {
                return settings;
            }
        }
    }
    AppSettings::default()
}

pub fn save_settings(settings: &AppSettings) {
    let path = get_settings_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(settings) {
        let _ = fs::write(&path, json);
    }
}

pub fn get_app_settings() -> &'static Arc<Mutex<AppSettings>> {
    APP_SETTINGS.get_or_init(|| Arc::new(Mutex::new(load_settings())))
}

pub fn initialize_runtime_from_settings() {
    let settings = get_app_settings().lock().unwrap().clone();

    // Sync runtime states with persistent settings
    *get_media_enabled().lock().unwrap() = settings.media_enabled;
}

pub fn update_setting<F: FnOnce(&mut AppSettings)>(f: F) {
    let settings = get_app_settings();
    let mut guard = settings.lock().unwrap();
    f(&mut guard);
    save_settings(&guard);
}

pub fn get_saved_devices_setting() -> HashMap<String, SavedDeviceInfo> {
    get_app_settings().lock().unwrap().saved_devices.clone()
}

pub fn save_device_to_settings(device_id: String, info: SavedDeviceInfo) {
    update_setting(|s| {
        s.saved_devices.insert(device_id, info);
    });
}

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
static PREVIEW_DATA: OnceLock<Arc<Mutex<Option<PreviewData>>>> = OnceLock::new();
static MEDIA_ENABLED: OnceLock<Arc<Mutex<bool>>> = OnceLock::new();
static CURRENT_MEDIA: OnceLock<Arc<Mutex<Option<RemoteMedia>>>> = OnceLock::new();
static LAST_REMOTE_MEDIA_DEVICE_ID: OnceLock<Arc<Mutex<Option<String>>>> = OnceLock::new();
static THUMBNAILS: OnceLock<Arc<Mutex<HashMap<String, Vec<u8>>>>> = OnceLock::new();
static THUMBNAILS_UPDATE: OnceLock<Arc<Mutex<std::time::Instant>>> = OnceLock::new();

// Telephony state
static PHONE_CONTACTS: OnceLock<Arc<Mutex<Vec<Contact>>>> = OnceLock::new();
static PHONE_CONVERSATIONS: OnceLock<Arc<Mutex<Vec<Conversation>>>> = OnceLock::new();
static PHONE_MESSAGES: OnceLock<Arc<Mutex<HashMap<String, Vec<SmsMessage>>>>> = OnceLock::new();
static PHONE_CALL_LOG: OnceLock<Arc<Mutex<Vec<CallLogEntry>>>> = OnceLock::new();
static PHONE_DATA_UPDATE: OnceLock<Arc<Mutex<std::time::Instant>>> = OnceLock::new();
static ACTIVE_CALL: OnceLock<Arc<Mutex<Option<ActiveCall>>>> = OnceLock::new();

// Track which device's phone data we have cached
static PHONE_DATA_DEVICE_ID: OnceLock<Arc<Mutex<Option<String>>>> = OnceLock::new();
// Track if initial sync has been done for each category
static PHONE_SYNC_DONE: OnceLock<Arc<Mutex<PhoneSyncState>>> = OnceLock::new();

#[derive(Debug, Clone, Default)]
pub struct PhoneSyncState {
    pub messages_synced: bool,
    pub calls_synced: bool,
    pub contacts_synced: bool,
}

pub fn get_media_enabled() -> &'static Arc<Mutex<bool>> {
    MEDIA_ENABLED.get_or_init(|| {
        // Initialize from persistent settings
        let settings = load_settings();
        Arc::new(Mutex::new(settings.media_enabled))
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct RemoteMedia {
    pub state: MediaState,
    pub source_device_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedDeviceInfo {
    pub name: String,
    pub ip: String,
    pub port: u16,
    pub device_type: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreviewData {
    pub filename: String,
    pub mime_type: String,
    pub data: Vec<u8>,
}

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

pub fn get_current_media() -> &'static Arc<Mutex<Option<RemoteMedia>>> {
    CURRENT_MEDIA.get_or_init(|| Arc::new(Mutex::new(None)))
}

pub fn get_last_remote_media_device_id() -> &'static Arc<Mutex<Option<String>>> {
    LAST_REMOTE_MEDIA_DEVICE_ID.get_or_init(|| Arc::new(Mutex::new(None)))
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
    if cfg!(target_os = "linux") && !get_notifications_enabled_setting() {
        return;
    }

    #[cfg(target_os = "linux")]
    {
        if get_notifications_enabled_setting() {
            let summary = title.to_string();
            let body = format!("{} {}", icon, message);
            std::thread::spawn(move || {
                if let Err(e) = notify_rust::Notification::new()
                    .summary(&summary)
                    .body(&body)
                    .show()
                {
                    tracing::warn!("Failed to show system notification: {}", e);
                }
            });
        }
    }

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

pub fn get_preview_data() -> &'static Arc<Mutex<Option<PreviewData>>> {
    PREVIEW_DATA.get_or_init(|| Arc::new(Mutex::new(None)))
}

pub fn get_thumbnails() -> &'static Arc<Mutex<HashMap<String, Vec<u8>>>> {
    THUMBNAILS.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

pub fn get_thumbnails_update() -> &'static Arc<Mutex<std::time::Instant>> {
    THUMBNAILS_UPDATE.get_or_init(|| Arc::new(Mutex::new(std::time::Instant::now())))
}

// Telephony getters
pub fn get_phone_contacts() -> &'static Arc<Mutex<Vec<Contact>>> {
    PHONE_CONTACTS.get_or_init(|| Arc::new(Mutex::new(Vec::new())))
}

pub fn get_phone_conversations() -> &'static Arc<Mutex<Vec<Conversation>>> {
    PHONE_CONVERSATIONS.get_or_init(|| Arc::new(Mutex::new(Vec::new())))
}

pub fn get_phone_messages() -> &'static Arc<Mutex<HashMap<String, Vec<SmsMessage>>>> {
    PHONE_MESSAGES.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

pub fn get_phone_call_log() -> &'static Arc<Mutex<Vec<CallLogEntry>>> {
    PHONE_CALL_LOG.get_or_init(|| Arc::new(Mutex::new(Vec::new())))
}

pub fn get_phone_data_update() -> &'static Arc<Mutex<std::time::Instant>> {
    PHONE_DATA_UPDATE.get_or_init(|| Arc::new(Mutex::new(std::time::Instant::now())))
}

pub fn set_phone_contacts(contacts: Vec<Contact>) {
    *get_phone_contacts().lock().unwrap() = contacts;
    *get_phone_data_update().lock().unwrap() = std::time::Instant::now();
}

pub fn set_phone_conversations(conversations: Vec<Conversation>) {
    *get_phone_conversations().lock().unwrap() = conversations;
    *get_phone_data_update().lock().unwrap() = std::time::Instant::now();
}

pub fn set_phone_messages(thread_id: String, messages: Vec<SmsMessage>) {
    get_phone_messages()
        .lock()
        .unwrap()
        .insert(thread_id, messages);
    *get_phone_data_update().lock().unwrap() = std::time::Instant::now();
}

pub fn set_phone_call_log(entries: Vec<CallLogEntry>) {
    *get_phone_call_log().lock().unwrap() = entries;
    *get_phone_data_update().lock().unwrap() = std::time::Instant::now();
}

// Active call state
pub fn get_active_call() -> &'static Arc<Mutex<Option<ActiveCall>>> {
    ACTIVE_CALL.get_or_init(|| Arc::new(Mutex::new(None)))
}

pub fn set_active_call(call: Option<ActiveCall>) {
    *get_active_call().lock().unwrap() = call;
    *get_phone_data_update().lock().unwrap() = std::time::Instant::now();
}

// Phone data device tracking
pub fn get_phone_data_device_id() -> &'static Arc<Mutex<Option<String>>> {
    PHONE_DATA_DEVICE_ID.get_or_init(|| Arc::new(Mutex::new(None)))
}

pub fn get_phone_sync_state() -> &'static Arc<Mutex<PhoneSyncState>> {
    PHONE_SYNC_DONE.get_or_init(|| Arc::new(Mutex::new(PhoneSyncState::default())))
}

pub fn set_phone_data_device(device_id: Option<String>) {
    let should_clear = {
        let mut id_guard = get_phone_data_device_id().lock().unwrap();
        if *id_guard != device_id {
            *id_guard = device_id;
            true
        } else {
            false
        }
    };

    if should_clear {
        // Device changed, clear cached data and sync state
        // We do this outside the id_guard lock to prevent deadlocks
        *get_phone_sync_state().lock().unwrap() = PhoneSyncState::default();
        *get_phone_contacts().lock().unwrap() = Vec::new();
        *get_phone_conversations().lock().unwrap() = Vec::new();
        *get_phone_messages().lock().unwrap() = HashMap::new();
        *get_phone_call_log().lock().unwrap() = Vec::new();
    }
}

pub fn mark_messages_synced() {
    get_phone_sync_state().lock().unwrap().messages_synced = true;
}

pub fn mark_calls_synced() {
    get_phone_sync_state().lock().unwrap().calls_synced = true;
}

pub fn mark_contacts_synced() {
    get_phone_sync_state().lock().unwrap().contacts_synced = true;
}

pub fn is_messages_synced() -> bool {
    get_phone_sync_state().lock().unwrap().messages_synced
}

pub fn is_calls_synced() -> bool {
    get_phone_sync_state().lock().unwrap().calls_synced
}

pub fn is_contacts_synced() -> bool {
    get_phone_sync_state().lock().unwrap().contacts_synced
}

// Settings accessors that use persistent storage
pub fn get_clipboard_sync_enabled() -> bool {
    get_app_settings().lock().unwrap().clipboard_sync_enabled
}

pub fn set_clipboard_sync_enabled(enabled: bool) {
    update_setting(|s| s.clipboard_sync_enabled = enabled);
}

pub fn get_media_enabled_setting() -> bool {
    get_app_settings().lock().unwrap().media_enabled
}

pub fn set_media_enabled_setting(enabled: bool) {
    update_setting(|s| s.media_enabled = enabled);
}

pub fn get_auto_sync_messages() -> bool {
    get_app_settings().lock().unwrap().auto_sync_messages
}

pub fn set_auto_sync_messages(enabled: bool) {
    update_setting(|s| s.auto_sync_messages = enabled);
}

pub fn get_auto_sync_calls() -> bool {
    get_app_settings().lock().unwrap().auto_sync_calls
}

pub fn set_auto_sync_calls(enabled: bool) {
    update_setting(|s| s.auto_sync_calls = enabled);
}

pub fn get_auto_sync_contacts() -> bool {
    get_app_settings().lock().unwrap().auto_sync_contacts
}

pub fn set_auto_sync_contacts(enabled: bool) {
    update_setting(|s| s.auto_sync_contacts = enabled);
}

pub fn get_notifications_enabled_setting() -> bool {
    get_app_settings().lock().unwrap().notifications_enabled
}

pub fn set_notifications_enabled_setting(enabled: bool) {
    update_setting(|s| s.notifications_enabled = enabled);
}

pub fn get_device_name_setting() -> Option<String> {
    get_app_settings().lock().unwrap().device_name.clone()
}

pub fn set_device_name_setting(name: String) {
    update_setting(|s| s.device_name = Some(name));
}
