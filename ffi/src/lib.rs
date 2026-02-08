#![allow(unsafe_code)]

use connected_core::{ConnectedClient, ConnectedError, ConnectedEvent, Device, DeviceType};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::net::IpAddr;
#[cfg(target_os = "android")]
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, OnceLock};
use tokio::runtime::Runtime;
use tracing::{error, info, warn};

#[cfg(target_os = "android")]
use jni::JNIEnv;
#[cfg(target_os = "android")]
use jni::objects::JObject;
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
// Android TLS init (rustls-platform-verifier)
// ============================================================================

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub extern "C" fn Java_com_connected_app_RustlsPlatformVerifier_init(
    mut env: JNIEnv,
    _this: JObject,
    context: JObject,
) {
    init_android_logging();
    if let Err(err) = rustls_platform_verifier::android::init_with_env(&mut env, context) {
        log::error!("rustls-platform-verifier init failed: {}", err);
    }
}

// ============================================================================
// Global State (The "Singleton" for FFI)
// ============================================================================

static RUNTIME: OnceLock<Runtime> = OnceLock::new();
static INSTANCE: OnceLock<RwLock<Option<Arc<ConnectedClient>>>> = OnceLock::new();

// Global Callbacks
static DISCOVERY_CALLBACK: RwLock<Option<Box<dyn DiscoveryCallback>>> = RwLock::new(None);
static CLIPBOARD_CALLBACK: RwLock<Option<Box<dyn ClipboardCallback>>> = RwLock::new(None);
static TRANSFER_CALLBACK: RwLock<Option<Box<dyn FileTransferCallback>>> = RwLock::new(None);
static TRANSFER_SIZES: OnceLock<RwLock<HashMap<String, u64>>> = OnceLock::new();
static PAIRING_CALLBACK: RwLock<Option<Box<dyn PairingCallback>>> = RwLock::new(None);
static UNPAIR_CALLBACK: RwLock<Option<Box<dyn UnpairCallback>>> = RwLock::new(None);
static MEDIA_CALLBACK: RwLock<Option<Box<dyn MediaControlCallback>>> = RwLock::new(None);
static TELEPHONY_CALLBACK: RwLock<Option<Box<dyn TelephonyCallback>>> = RwLock::new(None);

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

fn get_transfer_sizes() -> &'static RwLock<HashMap<String, u64>> {
    TRANSFER_SIZES.get_or_init(|| RwLock::new(HashMap::new()))
}

// ============================================================================
// UniFFI Types
// ============================================================================
//
// Note: Many types here mirror types in `connected_core`. This duplication is
// intentional and required by UniFFI - the FFI layer needs its own type
// definitions with UniFFI derive macros to generate language bindings.
// The `From` trait implementations below handle conversion between the two.
// ============================================================================

#[derive(Debug, Clone, uniffi::Enum)]
pub enum MediaCommand {
    Play,
    Pause,
    PlayPause,
    Next,
    Previous,
    Stop,
    VolumeUp,
    VolumeDown,
}

impl From<MediaCommand> for connected_core::MediaCommand {
    fn from(c: MediaCommand) -> Self {
        match c {
            MediaCommand::Play => connected_core::MediaCommand::Play,
            MediaCommand::Pause => connected_core::MediaCommand::Pause,
            MediaCommand::PlayPause => connected_core::MediaCommand::PlayPause,
            MediaCommand::Next => connected_core::MediaCommand::Next,
            MediaCommand::Previous => connected_core::MediaCommand::Previous,
            MediaCommand::Stop => connected_core::MediaCommand::Stop,
            MediaCommand::VolumeUp => connected_core::MediaCommand::VolumeUp,
            MediaCommand::VolumeDown => connected_core::MediaCommand::VolumeDown,
        }
    }
}

impl From<connected_core::MediaCommand> for MediaCommand {
    fn from(c: connected_core::MediaCommand) -> Self {
        match c {
            connected_core::MediaCommand::Play => MediaCommand::Play,
            connected_core::MediaCommand::Pause => MediaCommand::Pause,
            connected_core::MediaCommand::PlayPause => MediaCommand::PlayPause,
            connected_core::MediaCommand::Next => MediaCommand::Next,
            connected_core::MediaCommand::Previous => MediaCommand::Previous,
            connected_core::MediaCommand::Stop => MediaCommand::Stop,
            connected_core::MediaCommand::VolumeUp => MediaCommand::VolumeUp,
            connected_core::MediaCommand::VolumeDown => MediaCommand::VolumeDown,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct MediaState {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub playing: bool,
}

// ============================================================================
// Telephony Types
// ============================================================================

#[derive(Debug, Clone, uniffi::Enum)]
pub enum PhoneNumberType {
    Mobile,
    Home,
    Work,
    Main,
    Other,
}

impl From<PhoneNumberType> for connected_core::PhoneNumberType {
    fn from(t: PhoneNumberType) -> Self {
        match t {
            PhoneNumberType::Mobile => connected_core::PhoneNumberType::Mobile,
            PhoneNumberType::Home => connected_core::PhoneNumberType::Home,
            PhoneNumberType::Work => connected_core::PhoneNumberType::Work,
            PhoneNumberType::Main => connected_core::PhoneNumberType::Main,
            PhoneNumberType::Other => connected_core::PhoneNumberType::Other,
        }
    }
}

impl From<connected_core::PhoneNumberType> for PhoneNumberType {
    fn from(t: connected_core::PhoneNumberType) -> Self {
        match t {
            connected_core::PhoneNumberType::Mobile => PhoneNumberType::Mobile,
            connected_core::PhoneNumberType::Home => PhoneNumberType::Home,
            connected_core::PhoneNumberType::Work => PhoneNumberType::Work,
            connected_core::PhoneNumberType::Main => PhoneNumberType::Main,
            connected_core::PhoneNumberType::Other => PhoneNumberType::Other,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FfiPhoneNumber {
    pub number: String,
    pub label: PhoneNumberType,
}

impl From<FfiPhoneNumber> for connected_core::PhoneNumber {
    fn from(p: FfiPhoneNumber) -> Self {
        connected_core::PhoneNumber {
            number: p.number,
            label: p.label.into(),
        }
    }
}

impl From<connected_core::PhoneNumber> for FfiPhoneNumber {
    fn from(p: connected_core::PhoneNumber) -> Self {
        FfiPhoneNumber {
            number: p.number,
            label: p.label.into(),
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FfiContact {
    pub id: String,
    pub name: String,
    pub phone_numbers: Vec<FfiPhoneNumber>,
    pub emails: Vec<String>,
    pub photo: Option<String>,
    pub starred: bool,
}

impl From<FfiContact> for connected_core::Contact {
    fn from(c: FfiContact) -> Self {
        connected_core::Contact {
            id: c.id,
            name: c.name,
            phone_numbers: c.phone_numbers.into_iter().map(|p| p.into()).collect(),
            emails: c.emails,
            photo: c.photo,
            starred: c.starred,
        }
    }
}

impl From<connected_core::Contact> for FfiContact {
    fn from(c: connected_core::Contact) -> Self {
        FfiContact {
            id: c.id,
            name: c.name,
            phone_numbers: c.phone_numbers.into_iter().map(|p| p.into()).collect(),
            emails: c.emails,
            photo: c.photo,
            starred: c.starred,
        }
    }
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum SmsStatus {
    Pending,
    Sent,
    Delivered,
    Failed,
    Received,
}

impl From<SmsStatus> for connected_core::SmsStatus {
    fn from(s: SmsStatus) -> Self {
        match s {
            SmsStatus::Pending => connected_core::SmsStatus::Pending,
            SmsStatus::Sent => connected_core::SmsStatus::Sent,
            SmsStatus::Delivered => connected_core::SmsStatus::Delivered,
            SmsStatus::Failed => connected_core::SmsStatus::Failed,
            SmsStatus::Received => connected_core::SmsStatus::Received,
        }
    }
}

impl From<connected_core::SmsStatus> for SmsStatus {
    fn from(s: connected_core::SmsStatus) -> Self {
        match s {
            connected_core::SmsStatus::Pending => SmsStatus::Pending,
            connected_core::SmsStatus::Sent => SmsStatus::Sent,
            connected_core::SmsStatus::Delivered => SmsStatus::Delivered,
            connected_core::SmsStatus::Failed => SmsStatus::Failed,
            connected_core::SmsStatus::Received => SmsStatus::Received,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FfiMmsAttachment {
    pub id: String,
    pub content_type: String,
    pub filename: Option<String>,
    /// Base64 encoded data for small attachments, or a reference ID for larger ones
    pub data: Option<String>,
}

impl From<FfiMmsAttachment> for connected_core::MmsAttachment {
    fn from(a: FfiMmsAttachment) -> Self {
        connected_core::MmsAttachment {
            id: a.id,
            content_type: a.content_type,
            filename: a.filename,
            data: a.data,
        }
    }
}

impl From<connected_core::MmsAttachment> for FfiMmsAttachment {
    fn from(a: connected_core::MmsAttachment) -> Self {
        FfiMmsAttachment {
            id: a.id,
            content_type: a.content_type,
            filename: a.filename,
            data: a.data,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FfiSmsMessage {
    pub id: String,
    pub thread_id: String,
    pub address: String,
    pub contact_name: Option<String>,
    pub body: String,
    pub timestamp: u64,
    pub is_outgoing: bool,
    pub is_read: bool,
    pub status: SmsStatus,
    pub attachments: Vec<FfiMmsAttachment>,
}

impl From<FfiSmsMessage> for connected_core::SmsMessage {
    fn from(m: FfiSmsMessage) -> Self {
        connected_core::SmsMessage {
            id: m.id,
            thread_id: m.thread_id,
            address: m.address,
            contact_name: m.contact_name,
            body: m.body,
            timestamp: m.timestamp,
            is_outgoing: m.is_outgoing,
            is_read: m.is_read,
            status: m.status.into(),
            attachments: m.attachments.into_iter().map(|a| a.into()).collect(),
        }
    }
}

impl From<connected_core::SmsMessage> for FfiSmsMessage {
    fn from(m: connected_core::SmsMessage) -> Self {
        FfiSmsMessage {
            id: m.id,
            thread_id: m.thread_id,
            address: m.address,
            contact_name: m.contact_name,
            body: m.body,
            timestamp: m.timestamp,
            is_outgoing: m.is_outgoing,
            is_read: m.is_read,
            status: m.status.into(),
            attachments: m.attachments.into_iter().map(|a| a.into()).collect(),
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FfiConversation {
    pub id: String,
    pub addresses: Vec<String>,
    pub contact_names: Vec<String>,
    pub last_message: Option<String>,
    pub last_timestamp: u64,
    pub unread_count: u32,
}

impl From<FfiConversation> for connected_core::Conversation {
    fn from(c: FfiConversation) -> Self {
        connected_core::Conversation {
            id: c.id,
            addresses: c.addresses,
            contact_names: c.contact_names,
            last_message: c.last_message,
            last_timestamp: c.last_timestamp,
            unread_count: c.unread_count,
        }
    }
}

impl From<connected_core::Conversation> for FfiConversation {
    fn from(c: connected_core::Conversation) -> Self {
        FfiConversation {
            id: c.id,
            addresses: c.addresses,
            contact_names: c.contact_names,
            last_message: c.last_message,
            last_timestamp: c.last_timestamp,
            unread_count: c.unread_count,
        }
    }
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum CallType {
    Incoming,
    Outgoing,
    Missed,
    Rejected,
    Blocked,
    Voicemail,
}

impl From<CallType> for connected_core::CallType {
    fn from(t: CallType) -> Self {
        match t {
            CallType::Incoming => connected_core::CallType::Incoming,
            CallType::Outgoing => connected_core::CallType::Outgoing,
            CallType::Missed => connected_core::CallType::Missed,
            CallType::Rejected => connected_core::CallType::Rejected,
            CallType::Blocked => connected_core::CallType::Blocked,
            CallType::Voicemail => connected_core::CallType::Voicemail,
        }
    }
}

impl From<connected_core::CallType> for CallType {
    fn from(t: connected_core::CallType) -> Self {
        match t {
            connected_core::CallType::Incoming => CallType::Incoming,
            connected_core::CallType::Outgoing => CallType::Outgoing,
            connected_core::CallType::Missed => CallType::Missed,
            connected_core::CallType::Rejected => CallType::Rejected,
            connected_core::CallType::Blocked => CallType::Blocked,
            connected_core::CallType::Voicemail => CallType::Voicemail,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FfiCallLogEntry {
    pub id: String,
    pub number: String,
    pub contact_name: Option<String>,
    pub call_type: CallType,
    pub timestamp: u64,
    pub duration: u32,
    pub is_read: bool,
}

impl From<FfiCallLogEntry> for connected_core::CallLogEntry {
    fn from(e: FfiCallLogEntry) -> Self {
        connected_core::CallLogEntry {
            id: e.id,
            number: e.number,
            contact_name: e.contact_name,
            call_type: e.call_type.into(),
            timestamp: e.timestamp,
            duration: e.duration,
            is_read: e.is_read,
        }
    }
}

impl From<connected_core::CallLogEntry> for FfiCallLogEntry {
    fn from(e: connected_core::CallLogEntry) -> Self {
        FfiCallLogEntry {
            id: e.id,
            number: e.number,
            contact_name: e.contact_name,
            call_type: e.call_type.into(),
            timestamp: e.timestamp,
            duration: e.duration,
            is_read: e.is_read,
        }
    }
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum ActiveCallState {
    Ringing,
    Dialing,
    Connected,
    OnHold,
    Ended,
}

impl From<ActiveCallState> for connected_core::ActiveCallState {
    fn from(s: ActiveCallState) -> Self {
        match s {
            ActiveCallState::Ringing => connected_core::ActiveCallState::Ringing,
            ActiveCallState::Dialing => connected_core::ActiveCallState::Dialing,
            ActiveCallState::Connected => connected_core::ActiveCallState::Connected,
            ActiveCallState::OnHold => connected_core::ActiveCallState::OnHold,
            ActiveCallState::Ended => connected_core::ActiveCallState::Ended,
        }
    }
}

impl From<connected_core::ActiveCallState> for ActiveCallState {
    fn from(s: connected_core::ActiveCallState) -> Self {
        match s {
            connected_core::ActiveCallState::Ringing => ActiveCallState::Ringing,
            connected_core::ActiveCallState::Dialing => ActiveCallState::Dialing,
            connected_core::ActiveCallState::Connected => ActiveCallState::Connected,
            connected_core::ActiveCallState::OnHold => ActiveCallState::OnHold,
            connected_core::ActiveCallState::Ended => ActiveCallState::Ended,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FfiActiveCall {
    pub number: String,
    pub contact_name: Option<String>,
    pub state: ActiveCallState,
    pub duration: u32,
    pub is_incoming: bool,
}

impl From<FfiActiveCall> for connected_core::ActiveCall {
    fn from(c: FfiActiveCall) -> Self {
        connected_core::ActiveCall {
            number: c.number,
            contact_name: c.contact_name,
            state: c.state.into(),
            duration: c.duration,
            is_incoming: c.is_incoming,
        }
    }
}

impl From<connected_core::ActiveCall> for FfiActiveCall {
    fn from(c: connected_core::ActiveCall) -> Self {
        FfiActiveCall {
            number: c.number,
            contact_name: c.contact_name,
            state: c.state.into(),
            duration: c.duration,
            is_incoming: c.is_incoming,
        }
    }
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum CallAction {
    Answer,
    Reject,
    HangUp,
    Mute,
    Unmute,
    Hold,
    Unhold,
    SendDtmf { digit: String },
}

impl From<CallAction> for connected_core::CallAction {
    fn from(a: CallAction) -> Self {
        match a {
            CallAction::Answer => connected_core::CallAction::Answer,
            CallAction::Reject => connected_core::CallAction::Reject,
            CallAction::HangUp => connected_core::CallAction::HangUp,
            CallAction::Mute => connected_core::CallAction::Mute,
            CallAction::Unmute => connected_core::CallAction::Unmute,
            CallAction::Hold => connected_core::CallAction::Hold,
            CallAction::Unhold => connected_core::CallAction::Unhold,
            CallAction::SendDtmf { digit } => {
                connected_core::CallAction::SendDtmf(digit.chars().next().unwrap_or('0'))
            }
        }
    }
}

impl From<connected_core::CallAction> for CallAction {
    fn from(a: connected_core::CallAction) -> Self {
        match a {
            connected_core::CallAction::Answer => CallAction::Answer,
            connected_core::CallAction::Reject => CallAction::Reject,
            connected_core::CallAction::HangUp => CallAction::HangUp,
            connected_core::CallAction::Mute => CallAction::Mute,
            connected_core::CallAction::Unmute => CallAction::Unmute,
            connected_core::CallAction::Hold => CallAction::Hold,
            connected_core::CallAction::Unhold => CallAction::Unhold,
            connected_core::CallAction::SendDtmf(c) => CallAction::SendDtmf {
                digit: c.to_string(),
            },
        }
    }
}

impl From<MediaState> for connected_core::MediaState {
    fn from(s: MediaState) -> Self {
        Self {
            title: s.title,
            artist: s.artist,
            album: s.album,
            playing: s.playing,
        }
    }
}

impl From<connected_core::MediaState> for MediaState {
    fn from(s: connected_core::MediaState) -> Self {
        Self {
            title: s.title,
            artist: s.artist,
            album: s.album,
            playing: s.playing,
        }
    }
}

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
    fn on_compression_progress(
        &self,
        filename: String,
        current_file: String,
        files_processed: u64,
        total_files: u64,
        bytes_processed: u64,
        total_bytes: u64,
        speed_bytes_per_sec: u64,
    );
}

#[uniffi::export(callback_interface)]
pub trait BrowserDownloadCallback: Send + Sync {
    /// Called when download progress updates
    /// For file downloads: current_file is the filename
    /// For folder downloads: current_file shows the file currently being downloaded
    fn on_download_progress(&self, bytes_downloaded: u64, total_bytes: u64, current_file: String);
    fn on_download_completed(&self, total_bytes: u64);
    fn on_download_failed(&self, error_msg: String);
}

#[uniffi::export(callback_interface)]
pub trait ClipboardCallback: Send + Sync {
    fn on_clipboard_received(&self, text: String, from_device: String);
    fn on_clipboard_sent(&self, success: bool, error_msg: Option<String>);
}

#[uniffi::export(callback_interface)]
pub trait PairingCallback: Send + Sync {
    fn on_pairing_request(&self, device_name: String, fingerprint: String, device_id: String);
    fn on_pairing_rejected(&self, device_name: String, device_id: String);
    fn on_pairing_mode_changed(&self, enabled: bool);
}

#[uniffi::export(callback_interface)]
pub trait UnpairCallback: Send + Sync {
    fn on_device_unpaired(&self, device_id: String, device_name: String, reason: String);
}

#[uniffi::export(callback_interface)]
pub trait MediaControlCallback: Send + Sync {
    fn on_media_command(&self, from_device: String, command: MediaCommand);
    fn on_media_state_update(&self, from_device: String, state: MediaState);
}

#[uniffi::export(callback_interface)]
pub trait TelephonyCallback: Send + Sync {
    /// Called when contacts sync is requested
    fn on_contacts_sync_request(&self, from_device: String, from_ip: String, from_port: u16);
    /// Called when contacts are received
    fn on_contacts_received(&self, from_device: String, contacts: Vec<FfiContact>);
    /// Called when conversations sync is requested
    fn on_conversations_sync_request(&self, from_device: String, from_ip: String, from_port: u16);
    /// Called when conversations are received
    fn on_conversations_received(&self, from_device: String, conversations: Vec<FfiConversation>);
    /// Called when messages for a thread are requested
    fn on_messages_request(
        &self,
        from_device: String,
        from_ip: String,
        from_port: u16,
        thread_id: String,
        limit: u32,
    );
    /// Called when messages are received
    fn on_messages_received(
        &self,
        from_device: String,
        thread_id: String,
        messages: Vec<FfiSmsMessage>,
    );
    /// Called when a request to send SMS is received
    fn on_send_sms_request(
        &self,
        from_device: String,
        from_ip: String,
        from_port: u16,
        to: String,
        body: String,
    );
    /// Called when SMS send result is received
    fn on_sms_send_result(&self, success: bool, message_id: Option<String>, error: Option<String>);
    /// Called when a new SMS notification is received
    fn on_new_sms(&self, from_device: String, message: FfiSmsMessage);
    /// Called when call log is requested
    fn on_call_log_request(&self, from_device: String, from_ip: String, from_port: u16, limit: u32);
    /// Called when call log is received
    fn on_call_log_received(&self, from_device: String, entries: Vec<FfiCallLogEntry>);
    /// Called when a call initiation is requested
    fn on_initiate_call_request(
        &self,
        from_device: String,
        from_ip: String,
        from_port: u16,
        number: String,
    );
    /// Called when a call action is requested
    fn on_call_action_request(
        &self,
        from_device: String,
        from_ip: String,
        from_port: u16,
        action: CallAction,
    );
    /// Called when active call state is updated
    fn on_active_call_update(&self, from_device: String, call: Option<FfiActiveCall>);
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
    // Check if already initialized
    if INSTANCE.get_or_init(|| RwLock::new(None)).read().is_some() {
        return Err(ConnectedFfiError::InitializationError {
            msg: "Client already initialized".into(),
        });
    }

    init_android_logging();

    let runtime = get_runtime();

    // Parse device type
    // Parse device type
    let dtype = DeviceType::from_str(&device_type).unwrap_or(DeviceType::Unknown); // assuming helper exists or we implement logic
    // Actually DeviceType::from_str is available in core::device

    let path = if storage_path.is_empty() {
        None
    } else {
        Some(PathBuf::from(storage_path))
    };

    let client = runtime.block_on(async {
        #[cfg(target_os = "android")]
        {
            ConnectedClient::new_with_bind_all(device_name, dtype, bind_port, path).await
        }
        #[cfg(not(target_os = "android"))]
        {
            ConnectedClient::new(device_name, dtype, bind_port, path).await
        }
    })?;

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
    // Check if already initialized
    if INSTANCE.get_or_init(|| RwLock::new(None)).read().is_some() {
        return Err(ConnectedFfiError::InitializationError {
            msg: "Client already initialized".into(),
        });
    }

    init_android_logging();
    let runtime = get_runtime();
    // Parse device type
    let dtype = DeviceType::from_str(&device_type).unwrap_or(DeviceType::Unknown);
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
        #[cfg(target_os = "android")]
        {
            ConnectedClient::new_with_bind_ip(
                device_name,
                dtype,
                bind_port,
                ip,
                IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                path,
            )
            .await
        }
        #[cfg(not(target_os = "android"))]
        {
            ConnectedClient::new_with_ip(device_name, dtype, bind_port, ip, path).await
        }
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
                        id,
                        filename,
                        total_size,
                        ..
                    } => {
                        get_transfer_sizes().write().insert(id, total_size);
                        if let Some(cb) = TRANSFER_CALLBACK.read().as_ref() {
                            cb.on_transfer_starting(filename, total_size);
                        }
                    }
                    ConnectedEvent::TransferProgress {
                        id,
                        bytes_transferred,
                        total_size,
                        ..
                    } => {
                        get_transfer_sizes().write().insert(id, total_size);
                        if let Some(cb) = TRANSFER_CALLBACK.read().as_ref() {
                            cb.on_transfer_progress(bytes_transferred, total_size);
                        }
                    }
                    ConnectedEvent::TransferCompleted { id, filename } => {
                        let total_size = get_transfer_sizes().write().remove(&id).unwrap_or(0);
                        if let Some(cb) = TRANSFER_CALLBACK.read().as_ref() {
                            cb.on_transfer_completed(filename, total_size);
                        }
                    }
                    ConnectedEvent::TransferFailed { id, error } => {
                        get_transfer_sizes().write().remove(&id);
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
                    ConnectedEvent::PairingRejected {
                        device_name,
                        device_id,
                    } => {
                        if let Some(cb) = PAIRING_CALLBACK.read().as_ref() {
                            cb.on_pairing_rejected(device_name, device_id);
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
                        };
                        if let Some(cb) = UNPAIR_CALLBACK.read().as_ref() {
                            cb.on_device_unpaired(device_id, device_name, reason_str.to_string());
                        }
                    }
                    ConnectedEvent::MediaControl { from_device, event } => {
                        if let Some(cb) = MEDIA_CALLBACK.read().as_ref() {
                            match event {
                                connected_core::MediaControlMessage::Command(cmd) => {
                                    cb.on_media_command(from_device, cmd.into());
                                }
                                connected_core::MediaControlMessage::StateUpdate(state) => {
                                    cb.on_media_state_update(from_device, state.into());
                                }
                            }
                        }
                    }
                    ConnectedEvent::Telephony {
                        from_device,
                        from_ip,
                        from_port,
                        message,
                    } => {
                        if let Some(cb) = TELEPHONY_CALLBACK.read().as_ref() {
                            use connected_core::TelephonyMessage;
                            match message {
                                TelephonyMessage::ContactsSyncRequest => {
                                    cb.on_contacts_sync_request(from_device, from_ip, from_port);
                                }
                                TelephonyMessage::ContactsSyncResponse { contacts } => {
                                    let ffi_contacts: Vec<FfiContact> =
                                        contacts.into_iter().map(|c| c.into()).collect();
                                    cb.on_contacts_received(from_device, ffi_contacts);
                                }
                                TelephonyMessage::ConversationsSyncRequest => {
                                    cb.on_conversations_sync_request(
                                        from_device,
                                        from_ip.clone(),
                                        from_port,
                                    );
                                }
                                TelephonyMessage::ConversationsSyncResponse { conversations } => {
                                    let ffi_convos: Vec<FfiConversation> =
                                        conversations.into_iter().map(|c| c.into()).collect();
                                    cb.on_conversations_received(from_device, ffi_convos);
                                }
                                TelephonyMessage::MessagesRequest {
                                    thread_id, limit, ..
                                } => {
                                    cb.on_messages_request(
                                        from_device,
                                        from_ip.clone(),
                                        from_port,
                                        thread_id,
                                        limit,
                                    );
                                }
                                TelephonyMessage::MessagesResponse {
                                    thread_id,
                                    messages,
                                } => {
                                    let ffi_msgs: Vec<FfiSmsMessage> =
                                        messages.into_iter().map(|m| m.into()).collect();
                                    cb.on_messages_received(from_device, thread_id, ffi_msgs);
                                }
                                TelephonyMessage::SendSms { to, body } => {
                                    cb.on_send_sms_request(
                                        from_device,
                                        from_ip.clone(),
                                        from_port,
                                        to,
                                        body,
                                    );
                                }
                                TelephonyMessage::SmsSendResult {
                                    success,
                                    message_id,
                                    error,
                                } => {
                                    cb.on_sms_send_result(success, message_id, error);
                                }
                                TelephonyMessage::NewSmsNotification { message } => {
                                    cb.on_new_sms(from_device, message.into());
                                }
                                TelephonyMessage::CallLogRequest { limit, .. } => {
                                    cb.on_call_log_request(
                                        from_device,
                                        from_ip.clone(),
                                        from_port,
                                        limit,
                                    );
                                }
                                TelephonyMessage::CallLogResponse { entries } => {
                                    let ffi_entries: Vec<FfiCallLogEntry> =
                                        entries.into_iter().map(|e| e.into()).collect();
                                    cb.on_call_log_received(from_device, ffi_entries);
                                }
                                TelephonyMessage::InitiateCall { number } => {
                                    cb.on_initiate_call_request(
                                        from_device,
                                        from_ip.clone(),
                                        from_port,
                                        number,
                                    );
                                }
                                TelephonyMessage::CallAction { action } => {
                                    cb.on_call_action_request(
                                        from_device,
                                        from_ip.clone(),
                                        from_port,
                                        action.into(),
                                    );
                                }
                                TelephonyMessage::ActiveCallUpdate { call } => {
                                    cb.on_active_call_update(from_device, call.map(|c| c.into()));
                                }
                                TelephonyMessage::MarkMessagesRead { .. } => {
                                    // Handle mark as read if needed
                                }
                                TelephonyMessage::DeleteMessages { .. } => {
                                    // Handle delete messages if needed
                                }
                                TelephonyMessage::DeleteConversation { .. } => {
                                    // Handle delete conversation if needed
                                }
                            }
                        }
                    }
                    ConnectedEvent::PairingModeChanged(enabled) => {
                        if let Some(cb) = PAIRING_CALLBACK.read().as_ref() {
                            cb.on_pairing_mode_changed(enabled);
                        }
                    }
                    ConnectedEvent::CompressionProgress {
                        filename,
                        current_file,
                        files_processed,
                        total_files,
                        bytes_processed,
                        total_bytes,
                        speed_bytes_per_sec,
                    } => {
                        if let Some(cb) = TRANSFER_CALLBACK.read().as_ref() {
                            cb.on_compression_progress(
                                filename,
                                current_file,
                                files_processed,
                                total_files,
                                bytes_processed,
                                total_bytes,
                                speed_bytes_per_sec,
                            );
                        }
                    }
                    ConnectedEvent::Error(msg) => {
                        error!("Core error: {}", msg);
                    }
                },
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(
                        "Event listener lagged, skipped {} events. Increase capacity or process faster.",
                        skipped
                    );
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    info!("Event listener channel closed, stopping.");
                    break;
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
pub fn inject_proximity_device(
    device_id: String,
    device_name: String,
    device_type: String,
    ip: String,
    port: u16,
) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    // Parse device type
    let dtype = DeviceType::from_str(&device_type).unwrap_or(DeviceType::Unknown);
    let ip: IpAddr = ip.parse().map_err(|_| ConnectedFfiError::InvalidArgument {
        msg: "Invalid IP".into(),
    })?;

    client.inject_proximity_device(device_id, device_name, dtype, ip, port)?;
    Ok(())
}

#[uniffi::export]
pub fn stop_discovery() {
    // Clear the callback to stop sending events
    *DISCOVERY_CALLBACK.write() = None;

    // Also clear discovered devices cache so fresh discovery starts clean
    if let Ok(client) = get_client() {
        client.clear_discovered_devices();
    }
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
pub fn refresh_discovery() -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    client.refresh_discovery();
    Ok(())
}

#[uniffi::export]
pub fn set_download_directory(path: String) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    client.set_download_dir(PathBuf::from(path)).map_err(|e| {
        ConnectedFfiError::InitializationError {
            msg: format!("Failed to set download directory: {}", e),
        }
    })
}

#[uniffi::export]
pub fn get_download_directory() -> Result<String, ConnectedFfiError> {
    let client = get_client()?;
    Ok(client.get_download_dir().to_string_lossy().to_string())
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
    if !path.exists() {
        return Err(ConnectedFfiError::InvalidArgument {
            msg: format!("File not found: {:?}", path),
        });
    }

    get_runtime().spawn(async move {
        if let Err(e) = client.send_file(ip, target_port, path).await {
            error!("Failed to start file transfer: {}", e);
        }
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
    get_runtime().spawn(async move {
        client.set_pairing_mode(enabled);
    });
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
pub fn reject_pairing(device_id: String) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    get_runtime()
        .block_on(async { client.reject_pairing(&device_id).await })
        .map_err(Into::into)
}

#[uniffi::export]
pub fn unpair_device(fingerprint: String) -> Result<(), ConnectedFfiError> {
    // Unpair = disconnect but keep trust intact (can reconnect anytime without re-pairing)
    let client = get_client()?;
    get_runtime()
        .block_on(async { client.unpair_device(&fingerprint).await })
        .map_err(Into::into)
}

#[uniffi::export]
pub fn unpair_device_by_id(device_id: String) -> Result<(), ConnectedFfiError> {
    // Unpair = disconnect but keep trust intact (can reconnect anytime without re-pairing)
    let client = get_client()?;
    get_runtime()
        .block_on(async { client.unpair_device_by_id(&device_id).await })
        .map_err(Into::into)
}

#[uniffi::export]
pub fn forget_device(fingerprint: String) -> Result<(), ConnectedFfiError> {
    // Forget = completely remove trust (must re-pair to connect again)
    let client = get_client()?;
    get_runtime()
        .block_on(async { client.forget_device(&fingerprint).await })
        .map_err(Into::into)
}

#[uniffi::export]
pub fn forget_device_by_id(device_id: String) -> Result<(), ConnectedFfiError> {
    // Forget = completely remove trust (must re-pair to connect again)
    let client = get_client()?;
    get_runtime()
        .block_on(async { client.forget_device_by_id(&device_id).await })
        .map_err(Into::into)
}

#[uniffi::export]
pub fn is_device_forgotten(fingerprint: String) -> bool {
    match get_client() {
        Ok(client) => client.is_device_forgotten(&fingerprint),
        _ => false,
    }
}

#[uniffi::export]
pub fn is_device_trusted(device_id: String) -> bool {
    match get_client() {
        Ok(client) => client.is_device_trusted(&device_id),
        _ => false,
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
    get_transfer_sizes().write().clear();

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
    fn get_thumbnail(&self, path: String) -> Result<Vec<u8>, FilesystemError>;
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

    fn get_thumbnail(&self, path: &str) -> connected_core::Result<Vec<u8>> {
        if let Some(cb) = FS_PROVIDER.read().as_ref() {
            cb.get_thumbnail(path.to_string())
                .map_err(|e| connected_core::ConnectedError::Filesystem(e.to_string()))
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

#[uniffi::export]
pub fn request_download_file(
    target_ip: String,
    target_port: u16,
    remote_path: String,
    local_path: String,
) -> Result<u64, ConnectedFfiError> {
    let client = get_client()?;
    let ip: std::net::IpAddr =
        target_ip
            .parse()
            .map_err(|_| ConnectedFfiError::InvalidArgument {
                msg: "Invalid IP".into(),
            })?;

    // Block on async call - Caller must run this in background thread!
    let bytes = get_runtime().block_on(async {
        client
            .fs_download_file(ip, target_port, remote_path, PathBuf::from(local_path))
            .await
    })?;

    Ok(bytes)
}

#[uniffi::export]
pub fn request_download_file_with_progress(
    target_ip: String,
    target_port: u16,
    remote_path: String,
    local_path: String,
    callback: Box<dyn BrowserDownloadCallback>,
) {
    let ip: std::net::IpAddr = match target_ip.parse() {
        Ok(ip) => ip,
        Err(_) => {
            callback.on_download_failed("Invalid IP address".to_string());
            return;
        }
    };

    let client = match get_client() {
        Ok(c) => c,
        Err(e) => {
            callback.on_download_failed(format!("Not initialized: {:?}", e));
            return;
        }
    };

    let filename = PathBuf::from(&remote_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string();

    get_runtime().spawn(async move {
        let result = client
            .fs_download_file_with_progress(
                ip,
                target_port,
                remote_path,
                PathBuf::from(local_path),
                |bytes_downloaded, total_bytes| {
                    callback.on_download_progress(bytes_downloaded, total_bytes, filename.clone());
                },
            )
            .await;

        match result {
            Ok(total) => callback.on_download_completed(total),
            Err(e) => callback.on_download_failed(format!("{:?}", e)),
        }
    });
}

#[uniffi::export]
pub fn request_download_folder(
    target_ip: String,
    target_port: u16,
    remote_path: String,
    local_path: String,
    callback: Box<dyn BrowserDownloadCallback>,
) {
    let ip: std::net::IpAddr = match target_ip.parse() {
        Ok(ip) => ip,
        Err(_) => {
            callback.on_download_failed("Invalid IP address".to_string());
            return;
        }
    };

    let client = match get_client() {
        Ok(c) => c,
        Err(e) => {
            callback.on_download_failed(format!("Not initialized: {:?}", e));
            return;
        }
    };

    get_runtime().spawn(async move {
        let result = client
            .fs_download_folder_with_progress(
                ip,
                target_port,
                remote_path,
                PathBuf::from(local_path),
                |bytes_downloaded, total_bytes, current_file| {
                    callback.on_download_progress(
                        bytes_downloaded,
                        total_bytes,
                        current_file.to_string(),
                    );
                },
            )
            .await;

        match result {
            Ok(total) => callback.on_download_completed(total),
            Err(e) => callback.on_download_failed(format!("{:?}", e)),
        }
    });
}

#[uniffi::export]
pub fn request_get_thumbnail(
    target_ip: String,
    target_port: u16,
    path: String,
) -> Result<Vec<u8>, ConnectedFfiError> {
    let client = get_client()?;
    let ip: std::net::IpAddr =
        target_ip
            .parse()
            .map_err(|_| ConnectedFfiError::InvalidArgument {
                msg: "Invalid IP".into(),
            })?;

    let data =
        get_runtime().block_on(async { client.fs_get_thumbnail(ip, target_port, path).await })?;

    Ok(data)
}

#[uniffi::export]
pub fn register_media_control_callback(callback: Box<dyn MediaControlCallback>) {
    *MEDIA_CALLBACK.write() = Some(callback);
}

#[uniffi::export]
pub fn send_media_command(
    target_ip: String,
    target_port: u16,
    command: MediaCommand,
) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    let ip: std::net::IpAddr =
        target_ip
            .parse()
            .map_err(|_| ConnectedFfiError::InvalidArgument {
                msg: "Invalid IP".into(),
            })?;

    let cmd: connected_core::MediaCommand = command.into();
    let msg = connected_core::MediaControlMessage::Command(cmd);

    get_runtime().spawn(async move {
        let _ = client.send_media_control(ip, target_port, msg).await;
    });

    Ok(())
}

#[uniffi::export]
pub fn send_media_state(
    target_ip: String,
    target_port: u16,
    state: MediaState,
) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    let ip: std::net::IpAddr =
        target_ip
            .parse()
            .map_err(|_| ConnectedFfiError::InvalidArgument {
                msg: "Invalid IP".into(),
            })?;

    let s: connected_core::MediaState = state.into();
    let msg = connected_core::MediaControlMessage::StateUpdate(s);

    get_runtime().spawn(async move {
        let _ = client.send_media_control(ip, target_port, msg).await;
    });

    Ok(())
}

// ============================================================================
// Telephony Functions
// ============================================================================

#[uniffi::export]
pub fn register_telephony_callback(callback: Box<dyn TelephonyCallback>) {
    *TELEPHONY_CALLBACK.write() = Some(callback);
}

#[uniffi::export]
pub fn request_contacts_sync(target_ip: String, target_port: u16) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    let ip: std::net::IpAddr =
        target_ip
            .parse()
            .map_err(|_| ConnectedFfiError::InvalidArgument {
                msg: "Invalid IP".into(),
            })?;

    let msg = connected_core::TelephonyMessage::ContactsSyncRequest;

    get_runtime().spawn(async move {
        let _ = client.send_telephony(ip, target_port, msg).await;
    });

    Ok(())
}

#[uniffi::export]
pub fn send_contacts(
    target_ip: String,
    target_port: u16,
    contacts: Vec<FfiContact>,
) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    let ip: std::net::IpAddr =
        target_ip
            .parse()
            .map_err(|_| ConnectedFfiError::InvalidArgument {
                msg: "Invalid IP".into(),
            })?;

    let core_contacts: Vec<connected_core::Contact> =
        contacts.into_iter().map(|c| c.into()).collect();
    let msg = connected_core::TelephonyMessage::ContactsSyncResponse {
        contacts: core_contacts,
    };

    get_runtime().spawn(async move {
        let _ = client.send_telephony(ip, target_port, msg).await;
    });

    Ok(())
}

#[uniffi::export]
pub fn request_conversations_sync(
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

    let msg = connected_core::TelephonyMessage::ConversationsSyncRequest;

    get_runtime().spawn(async move {
        let _ = client.send_telephony(ip, target_port, msg).await;
    });

    Ok(())
}

#[uniffi::export]
pub fn send_conversations(
    target_ip: String,
    target_port: u16,
    conversations: Vec<FfiConversation>,
) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    let ip: std::net::IpAddr =
        target_ip
            .parse()
            .map_err(|_| ConnectedFfiError::InvalidArgument {
                msg: "Invalid IP".into(),
            })?;

    let core_convos: Vec<connected_core::Conversation> =
        conversations.into_iter().map(|c| c.into()).collect();
    let msg = connected_core::TelephonyMessage::ConversationsSyncResponse {
        conversations: core_convos,
    };

    get_runtime().spawn(async move {
        let _ = client.send_telephony(ip, target_port, msg).await;
    });

    Ok(())
}

#[uniffi::export]
pub fn request_messages(
    target_ip: String,
    target_port: u16,
    thread_id: String,
    limit: u32,
) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    let ip: std::net::IpAddr =
        target_ip
            .parse()
            .map_err(|_| ConnectedFfiError::InvalidArgument {
                msg: "Invalid IP".into(),
            })?;

    let msg = connected_core::TelephonyMessage::MessagesRequest {
        thread_id,
        limit,
        before_timestamp: None,
    };

    get_runtime().spawn(async move {
        let _ = client.send_telephony(ip, target_port, msg).await;
    });

    Ok(())
}

#[uniffi::export]
pub fn send_messages(
    target_ip: String,
    target_port: u16,
    thread_id: String,
    messages: Vec<FfiSmsMessage>,
) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    let ip: std::net::IpAddr =
        target_ip
            .parse()
            .map_err(|_| ConnectedFfiError::InvalidArgument {
                msg: "Invalid IP".into(),
            })?;

    let core_msgs: Vec<connected_core::SmsMessage> =
        messages.into_iter().map(|m| m.into()).collect();
    let msg = connected_core::TelephonyMessage::MessagesResponse {
        thread_id,
        messages: core_msgs,
    };

    get_runtime().spawn(async move {
        let _ = client.send_telephony(ip, target_port, msg).await;
    });

    Ok(())
}

#[uniffi::export]
pub fn send_sms(
    target_ip: String,
    target_port: u16,
    to: String,
    body: String,
) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    let ip: std::net::IpAddr =
        target_ip
            .parse()
            .map_err(|_| ConnectedFfiError::InvalidArgument {
                msg: "Invalid IP".into(),
            })?;

    let msg = connected_core::TelephonyMessage::SendSms { to, body };

    get_runtime().spawn(async move {
        let _ = client.send_telephony(ip, target_port, msg).await;
    });

    Ok(())
}

#[uniffi::export]
pub fn send_sms_send_result(
    target_ip: String,
    target_port: u16,
    success: bool,
    message_id: Option<String>,
    error: Option<String>,
) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    let ip: std::net::IpAddr =
        target_ip
            .parse()
            .map_err(|_| ConnectedFfiError::InvalidArgument {
                msg: "Invalid IP".into(),
            })?;

    let msg = connected_core::TelephonyMessage::SmsSendResult {
        success,
        message_id,
        error,
    };

    get_runtime().spawn(async move {
        let _ = client.send_telephony(ip, target_port, msg).await;
    });

    Ok(())
}

#[uniffi::export]
pub fn notify_new_sms(
    target_ip: String,
    target_port: u16,
    message: FfiSmsMessage,
) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    let ip: std::net::IpAddr =
        target_ip
            .parse()
            .map_err(|_| ConnectedFfiError::InvalidArgument {
                msg: "Invalid IP".into(),
            })?;

    let core_msg: connected_core::SmsMessage = message.into();
    let msg = connected_core::TelephonyMessage::NewSmsNotification { message: core_msg };

    get_runtime().spawn(async move {
        let _ = client.send_telephony(ip, target_port, msg).await;
    });

    Ok(())
}

#[uniffi::export]
pub fn request_call_log(
    target_ip: String,
    target_port: u16,
    limit: u32,
) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    let ip: std::net::IpAddr =
        target_ip
            .parse()
            .map_err(|_| ConnectedFfiError::InvalidArgument {
                msg: "Invalid IP".into(),
            })?;

    let msg = connected_core::TelephonyMessage::CallLogRequest {
        limit,
        before_timestamp: None,
    };

    get_runtime().spawn(async move {
        let _ = client.send_telephony(ip, target_port, msg).await;
    });

    Ok(())
}

#[uniffi::export]
pub fn send_call_log(
    target_ip: String,
    target_port: u16,
    entries: Vec<FfiCallLogEntry>,
) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    let ip: std::net::IpAddr =
        target_ip
            .parse()
            .map_err(|_| ConnectedFfiError::InvalidArgument {
                msg: "Invalid IP".into(),
            })?;

    let core_entries: Vec<connected_core::CallLogEntry> =
        entries.into_iter().map(|e| e.into()).collect();
    let msg = connected_core::TelephonyMessage::CallLogResponse {
        entries: core_entries,
    };

    get_runtime().spawn(async move {
        let _ = client.send_telephony(ip, target_port, msg).await;
    });

    Ok(())
}

#[uniffi::export]
pub fn initiate_call(
    target_ip: String,
    target_port: u16,
    number: String,
) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    let ip: std::net::IpAddr =
        target_ip
            .parse()
            .map_err(|_| ConnectedFfiError::InvalidArgument {
                msg: "Invalid IP".into(),
            })?;

    let msg = connected_core::TelephonyMessage::InitiateCall { number };

    get_runtime().spawn(async move {
        let _ = client.send_telephony(ip, target_port, msg).await;
    });

    Ok(())
}

#[uniffi::export]
pub fn send_call_action(
    target_ip: String,
    target_port: u16,
    action: CallAction,
) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    let ip: std::net::IpAddr =
        target_ip
            .parse()
            .map_err(|_| ConnectedFfiError::InvalidArgument {
                msg: "Invalid IP".into(),
            })?;

    let core_action: connected_core::CallAction = action.into();
    let msg = connected_core::TelephonyMessage::CallAction {
        action: core_action,
    };

    get_runtime().spawn(async move {
        let _ = client.send_telephony(ip, target_port, msg).await;
    });

    Ok(())
}

#[uniffi::export]
pub fn send_active_call_update(
    target_ip: String,
    target_port: u16,
    call: Option<FfiActiveCall>,
) -> Result<(), ConnectedFfiError> {
    let client = get_client()?;
    let ip: std::net::IpAddr =
        target_ip
            .parse()
            .map_err(|_| ConnectedFfiError::InvalidArgument {
                msg: "Invalid IP".into(),
            })?;

    let core_call = call.map(|c| c.into());
    let msg = connected_core::TelephonyMessage::ActiveCallUpdate { call: core_call };

    get_runtime().spawn(async move {
        let _ = client.send_telephony(ip, target_port, msg).await;
    });

    Ok(())
}

// ============================================================================
// Update Support
// ============================================================================

#[derive(Debug, Clone, uniffi::Record)]
pub struct UpdateInfo {
    pub has_update: bool,
    pub latest_version: String,
    pub current_version: String,
    pub download_url: Option<String>,
    pub release_notes: Option<String>,
}

impl From<connected_core::UpdateInfo> for UpdateInfo {
    fn from(u: connected_core::UpdateInfo) -> Self {
        Self {
            has_update: u.has_update,
            latest_version: u.latest_version,
            current_version: u.current_version,
            download_url: u.download_url,
            release_notes: u.release_notes,
        }
    }
}

#[uniffi::export]
pub fn check_for_updates(
    current_version: String,
    platform: String,
) -> Result<UpdateInfo, ConnectedFfiError> {
    get_runtime()
        .block_on(async {
            connected_core::UpdateChecker::check_for_updates(current_version, platform).await
        })
        .map(|u| u.into())
        .map_err(|e| ConnectedFfiError::ConnectionError { msg: e.to_string() })
}

uniffi::setup_scaffolding!();
