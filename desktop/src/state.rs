use connected_core::filesystem::FsEntry;
use connected_core::telephony::{ActiveCall, CallLogEntry, Contact, Conversation, SmsMessage};
use connected_core::{Device, MediaState, UpdateInfo};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::panic::Location;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// Counter for tracking poison recovery events (useful for telemetry/debugging)
static POISON_RECOVERY_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Extension trait for `Mutex` that handles poisoning gracefully.
///
/// When a thread panics while holding a `std::sync::Mutex`, the lock becomes
/// "poisoned" and all subsequent `lock()` calls return `Err`.  Without
/// recovery this causes a cascade of panics across every thread that touches
/// the same mutex.
///
/// **Debug builds:** panic immediately so the root-cause bug surfaces early.
/// **Release builds:** recover the inner data and continue.  The data may be
/// in an inconsistent state, so the recovery counter should be monitored.
///
/// All methods are annotated with `#[track_caller]` so the log messages
/// report the *caller's* source location rather than this trait impl.
pub trait LockOrRecover<T> {
    fn lock_or_recover(&self) -> std::sync::MutexGuard<'_, T>;
}

/// Get the count of mutex poison recoveries (useful for monitoring/telemetry).
#[allow(dead_code)]
pub fn get_poison_recovery_count() -> usize {
    POISON_RECOVERY_COUNT.load(Ordering::Relaxed)
}

impl<T> LockOrRecover<T> for Mutex<T> {
    #[track_caller]
    fn lock_or_recover(&self) -> std::sync::MutexGuard<'_, T> {
        match self.lock() {
            Ok(guard) => guard,
            #[allow(unused_variables)]
            Err(poisoned) => {
                // A thread panicked while holding the lock.
                // This indicates a serious bug — data may be inconsistent.
                let count = POISON_RECOVERY_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                let caller = Location::caller();

                tracing::error!(
                    "Mutex was poisoned (recovery #{count})! A panic occurred while \
                     holding the lock. Data may be inconsistent. \
                     Caller: {file}:{line}",
                    count = count,
                    file = caller.file(),
                    line = caller.line(),
                );

                // In debug builds, fail loudly to surface the bug early.
                #[cfg(debug_assertions)]
                {
                    tracing::error!("Backtrace: {:?}", std::backtrace::Backtrace::capture());
                    panic!(
                        "Mutex poisoned at {}:{} — failing in debug mode to surface the \
                         underlying bug (recovery #{})",
                        caller.file(),
                        caller.line(),
                        count,
                    );
                }

                // In release builds, recover to prevent cascading panics.
                #[cfg(not(debug_assertions))]
                {
                    tracing::warn!(
                        "Recovering from poisoned mutex in release mode (recovery #{count}, \
                         caller: {file}:{line}). Application state may be inconsistent.",
                        count = count,
                        file = caller.file(),
                        line = caller.line(),
                    );
                    poisoned.into_inner()
                }
            }
        }
    }
}

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
    if path.exists()
        && let Ok(contents) = fs::read_to_string(&path)
        && let Ok(settings) = serde_json::from_str(&contents)
    {
        return settings;
    }
    AppSettings::default()
}

pub fn save_settings(settings: &AppSettings) {
    let path = get_settings_path();
    if let Some(parent) = path.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        tracing::error!("Failed to create settings directory: {}", e);
        return;
    }
    let json = match serde_json::to_string_pretty(settings) {
        Ok(j) => j,
        Err(e) => {
            tracing::error!("Failed to serialize settings: {}", e);
            return;
        }
    };

    // Atomic write: write to temp file, set permissions, fsync, then rename.
    // This prevents a half-written settings file if the process crashes mid-write.
    let tmp_path = path.with_extension("json.tmp");
    if let Err(e) = fs::write(&tmp_path, &json) {
        tracing::error!("Failed to write settings temp file: {}", e);
        return;
    }

    // Restrict permissions to owner-only (0600) to protect settings data
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        if let Err(e) = fs::set_permissions(&tmp_path, perms) {
            tracing::warn!("Failed to set permissions on settings temp file: {}", e);
        }
    }

    // fsync to ensure data is durable before rename
    match fs::File::open(&tmp_path) {
        Ok(f) => {
            if let Err(e) = f.sync_all() {
                tracing::warn!("Failed to fsync settings temp file: {}", e);
            }
        }
        Err(e) => {
            tracing::warn!("Failed to open settings temp file for fsync: {}", e);
        }
    }

    if let Err(e) = fs::rename(&tmp_path, &path) {
        tracing::error!("Failed to rename settings temp file: {}", e);
    }
}

pub fn get_app_settings() -> &'static Arc<Mutex<AppSettings>> {
    APP_SETTINGS.get_or_init(|| Arc::new(Mutex::new(load_settings())))
}

pub fn update_setting<F: FnOnce(&mut AppSettings)>(f: F) {
    let settings = get_app_settings();
    let mut guard = settings.lock_or_recover();
    f(&mut guard);
    save_settings(&guard);
}

pub fn get_saved_devices_setting() -> HashMap<String, SavedDeviceInfo> {
    get_app_settings().lock_or_recover().saved_devices.clone()
}

pub fn save_device_to_settings(device_id: String, info: SavedDeviceInfo) {
    update_setting(|s| {
        s.saved_devices.insert(device_id, info);
    });
}

pub fn remove_device_from_settings(device_id: &str) {
    update_setting(|s| {
        s.saved_devices.remove(device_id);
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
    Compressing {
        filename: String,
        current_file: String,
        files_processed: u64,
        total_files: u64,
        bytes_processed: u64,
        total_bytes: u64,
        speed_bytes_per_sec: u64,
    },
    Starting {
        filename: String,
    },
    InProgress {
        filename: String,
        percent: f32,
    },
    Completed {
        filename: String,
    },
    Failed {
        error: String,
    },
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
/// Stores the actual content of the last clipboard received from a remote device.
/// Used to prevent echo loops where we'd re-broadcast clipboard content we just received.
static LAST_REMOTE_CLIPBOARD_CONTENT: OnceLock<Arc<Mutex<String>>> = OnceLock::new();
static PAIRING_REQUESTS: OnceLock<Arc<Mutex<Vec<PairingRequest>>>> = OnceLock::new();
static PENDING_PAIRINGS: OnceLock<Arc<Mutex<HashSet<String>>>> = OnceLock::new();
static FILE_TRANSFER_REQUESTS: OnceLock<Arc<Mutex<HashMap<String, FileTransferRequest>>>> =
    OnceLock::new();
static REMOTE_FILES: OnceLock<Arc<Mutex<Option<Vec<FsEntry>>>>> = OnceLock::new();
static REMOTE_PATH: OnceLock<Arc<Mutex<String>>> = OnceLock::new();
static REMOTE_FILES_UPDATE: OnceLock<Arc<Mutex<std::time::Instant>>> = OnceLock::new();
static PREVIEW_DATA: OnceLock<Arc<Mutex<Option<PreviewData>>>> = OnceLock::new();
static MEDIA_ENABLED: OnceLock<Arc<Mutex<bool>>> = OnceLock::new();
static PAIRING_MODE: OnceLock<Arc<Mutex<bool>>> = OnceLock::new();
static CURRENT_MEDIA: OnceLock<Arc<Mutex<Option<RemoteMedia>>>> = OnceLock::new();
static LAST_REMOTE_MEDIA_DEVICE_ID: OnceLock<Arc<Mutex<Option<String>>>> = OnceLock::new();
static THUMBNAILS: OnceLock<ThumbnailsMap> = OnceLock::new();
static THUMBNAILS_UPDATE: OnceLock<Arc<Mutex<std::time::Instant>>> = OnceLock::new();
static UPDATE_INFO: OnceLock<Arc<Mutex<Option<UpdateInfo>>>> = OnceLock::new();
static SDK_INITIALIZED: OnceLock<Arc<Mutex<bool>>> = OnceLock::new();
static DISCOVERY_ACTIVE: OnceLock<Arc<Mutex<bool>>> = OnceLock::new();

type ThumbnailsMap = Arc<Mutex<HashMap<String, Vec<u8>>>>;
type MessagesMap = Arc<Mutex<HashMap<String, Vec<SmsMessage>>>>;

// Telephony state
static PHONE_CONTACTS: OnceLock<Arc<Mutex<Vec<Contact>>>> = OnceLock::new();
static PHONE_CONVERSATIONS: OnceLock<Arc<Mutex<Vec<Conversation>>>> = OnceLock::new();
static PHONE_MESSAGES: OnceLock<MessagesMap> = OnceLock::new();
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

pub fn get_sdk_initialized() -> &'static Arc<Mutex<bool>> {
    SDK_INITIALIZED.get_or_init(|| Arc::new(Mutex::new(false)))
}

pub fn set_sdk_initialized(value: bool) {
    *get_sdk_initialized().lock_or_recover() = value;
}

pub fn is_sdk_initialized() -> bool {
    *get_sdk_initialized().lock_or_recover()
}

pub fn get_discovery_active() -> &'static Arc<Mutex<bool>> {
    DISCOVERY_ACTIVE.get_or_init(|| Arc::new(Mutex::new(false)))
}

pub fn set_discovery_active(value: bool) {
    *get_discovery_active().lock_or_recover() = value;
}

pub fn is_discovery_active() -> bool {
    *get_discovery_active().lock_or_recover()
}

pub fn get_media_enabled() -> &'static Arc<Mutex<bool>> {
    MEDIA_ENABLED.get_or_init(|| {
        // Initialize from persistent settings
        let settings = load_settings();
        Arc::new(Mutex::new(settings.media_enabled))
    })
}

pub fn get_pairing_mode_state() -> &'static Arc<Mutex<bool>> {
    PAIRING_MODE.get_or_init(|| Arc::new(Mutex::new(false)))
}

pub fn set_pairing_mode_state(enabled: bool) {
    *get_pairing_mode_state().lock_or_recover() = enabled;
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

/// Set transfer status with auto-reset after completion or failure.
/// After Completed or Failed status, automatically resets to Idle after a delay.
pub fn set_transfer_status(status: TransferStatus) {
    let should_auto_reset = matches!(
        status,
        TransferStatus::Completed { .. } | TransferStatus::Failed { .. }
    );

    *get_transfer_status().lock_or_recover() = status;

    if should_auto_reset {
        std::thread::spawn(|| {
            // Reset to Idle after 5 seconds
            std::thread::sleep(std::time::Duration::from_secs(5));
            let mut guard = get_transfer_status().lock_or_recover();
            // Only reset if still in Completed or Failed state (not if a new transfer started)
            if matches!(
                *guard,
                TransferStatus::Completed { .. } | TransferStatus::Failed { .. }
            ) {
                *guard = TransferStatus::Idle;
            }
        });
    }
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

pub fn get_last_remote_clipboard_content() -> &'static Arc<Mutex<String>> {
    LAST_REMOTE_CLIPBOARD_CONTENT.get_or_init(|| Arc::new(Mutex::new(String::new())))
}

pub fn set_last_remote_clipboard_content(content: String) {
    *get_last_remote_clipboard_content().lock_or_recover() = content;
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
    let mut requests = get_file_transfer_requests().lock_or_recover();
    requests.insert(request.id.clone(), request);
}

pub fn remove_file_transfer_request(id: &str) -> Option<FileTransferRequest> {
    let mut requests = get_file_transfer_requests().lock_or_recover();
    requests.remove(id)
}

/// Cleanup old transfer requests that have been pending for too long (5 minutes).
/// This prevents unbounded growth of the transfer requests map.
pub fn cleanup_old_transfer_requests() {
    let mut requests = get_file_transfer_requests().lock_or_recover();
    let now = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(300); // 5 minutes
    requests.retain(|_, req| now.duration_since(req.timestamp) < timeout);
}

pub fn add_notification(title: &str, message: &str, icon: &'static str) {
    if (cfg!(target_os = "linux") || cfg!(target_os = "windows"))
        && !get_notifications_enabled_setting()
    {
        return;
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
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

    let mut counter = get_notification_counter().lock_or_recover();
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

    let mut notifications = get_notifications().lock_or_recover();
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

pub fn get_thumbnails() -> &'static ThumbnailsMap {
    THUMBNAILS.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

pub fn get_thumbnails_update() -> &'static Arc<Mutex<std::time::Instant>> {
    THUMBNAILS_UPDATE.get_or_init(|| Arc::new(Mutex::new(std::time::Instant::now())))
}

pub fn get_update_info() -> &'static Arc<Mutex<Option<UpdateInfo>>> {
    UPDATE_INFO.get_or_init(|| Arc::new(Mutex::new(None)))
}

// Telephony getters
pub fn get_phone_contacts() -> &'static Arc<Mutex<Vec<Contact>>> {
    PHONE_CONTACTS.get_or_init(|| Arc::new(Mutex::new(Vec::new())))
}

pub fn get_phone_conversations() -> &'static Arc<Mutex<Vec<Conversation>>> {
    PHONE_CONVERSATIONS.get_or_init(|| Arc::new(Mutex::new(Vec::new())))
}

pub fn get_phone_messages() -> &'static MessagesMap {
    PHONE_MESSAGES.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

pub fn get_phone_call_log() -> &'static Arc<Mutex<Vec<CallLogEntry>>> {
    PHONE_CALL_LOG.get_or_init(|| Arc::new(Mutex::new(Vec::new())))
}

pub fn get_phone_data_update() -> &'static Arc<Mutex<std::time::Instant>> {
    PHONE_DATA_UPDATE.get_or_init(|| Arc::new(Mutex::new(std::time::Instant::now())))
}

pub fn set_phone_contacts(contacts: Vec<Contact>) {
    *get_phone_contacts().lock_or_recover() = contacts;
    *get_phone_data_update().lock_or_recover() = std::time::Instant::now();
}

pub fn set_phone_conversations(conversations: Vec<Conversation>) {
    *get_phone_conversations().lock_or_recover() = conversations;
    *get_phone_data_update().lock_or_recover() = std::time::Instant::now();
}

pub fn set_phone_messages(thread_id: String, messages: Vec<SmsMessage>) {
    get_phone_messages()
        .lock_or_recover()
        .insert(thread_id, messages);
    *get_phone_data_update().lock_or_recover() = std::time::Instant::now();
}

pub fn set_phone_call_log(entries: Vec<CallLogEntry>) {
    *get_phone_call_log().lock_or_recover() = entries;
    *get_phone_data_update().lock_or_recover() = std::time::Instant::now();
}

// Active call state
pub fn get_active_call() -> &'static Arc<Mutex<Option<ActiveCall>>> {
    ACTIVE_CALL.get_or_init(|| Arc::new(Mutex::new(None)))
}

pub fn set_active_call(call: Option<ActiveCall>) {
    *get_active_call().lock_or_recover() = call;
    *get_phone_data_update().lock_or_recover() = std::time::Instant::now();
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
        let mut id_guard = get_phone_data_device_id().lock_or_recover();
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
        *get_phone_sync_state().lock_or_recover() = PhoneSyncState::default();
        *get_phone_contacts().lock_or_recover() = Vec::new();
        *get_phone_conversations().lock_or_recover() = Vec::new();
        *get_phone_messages().lock_or_recover() = HashMap::new();
        *get_phone_call_log().lock_or_recover() = Vec::new();
    }
}

pub fn mark_messages_synced() {
    get_phone_sync_state().lock_or_recover().messages_synced = true;
}

pub fn mark_calls_synced() {
    get_phone_sync_state().lock_or_recover().calls_synced = true;
}

pub fn mark_contacts_synced() {
    get_phone_sync_state().lock_or_recover().contacts_synced = true;
}

pub fn is_messages_synced() -> bool {
    get_phone_sync_state().lock_or_recover().messages_synced
}

pub fn is_calls_synced() -> bool {
    get_phone_sync_state().lock_or_recover().calls_synced
}

pub fn is_contacts_synced() -> bool {
    get_phone_sync_state().lock_or_recover().contacts_synced
}

// Settings accessors that use persistent storage
pub fn get_clipboard_sync_enabled() -> bool {
    get_app_settings().lock_or_recover().clipboard_sync_enabled
}

pub fn set_clipboard_sync_enabled(enabled: bool) {
    update_setting(|s| s.clipboard_sync_enabled = enabled);
}

pub fn get_media_enabled_setting() -> bool {
    get_app_settings().lock_or_recover().media_enabled
}

pub fn set_media_enabled_setting(enabled: bool) {
    update_setting(|s| s.media_enabled = enabled);
}

pub fn get_auto_sync_messages() -> bool {
    get_app_settings().lock_or_recover().auto_sync_messages
}

pub fn set_auto_sync_messages(enabled: bool) {
    update_setting(|s| s.auto_sync_messages = enabled);
}

pub fn get_auto_sync_calls() -> bool {
    get_app_settings().lock_or_recover().auto_sync_calls
}

pub fn set_auto_sync_calls(enabled: bool) {
    update_setting(|s| s.auto_sync_calls = enabled);
}

pub fn get_auto_sync_contacts() -> bool {
    get_app_settings().lock_or_recover().auto_sync_contacts
}

pub fn set_auto_sync_contacts(enabled: bool) {
    update_setting(|s| s.auto_sync_contacts = enabled);
}

pub fn get_notifications_enabled_setting() -> bool {
    get_app_settings().lock_or_recover().notifications_enabled
}

pub fn set_notifications_enabled_setting(enabled: bool) {
    update_setting(|s| s.notifications_enabled = enabled);
}

pub fn get_device_name_setting() -> Option<String> {
    get_app_settings().lock_or_recover().device_name.clone()
}

pub fn set_device_name_setting(name: String) {
    update_setting(|s| s.device_name = Some(name));
}
