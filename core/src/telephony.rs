use serde::{Deserialize, Serialize};

/// Represents a phone contact
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Contact {
    /// Unique identifier for the contact
    pub id: String,
    /// Display name
    pub name: String,
    /// Phone numbers associated with this contact
    pub phone_numbers: Vec<PhoneNumber>,
    /// Email addresses
    pub emails: Vec<String>,
    /// Optional photo as base64 encoded data
    pub photo: Option<String>,
    /// Whether this contact is starred/favorite
    pub starred: bool,
}

/// Phone number with type label
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PhoneNumber {
    pub number: String,
    pub label: PhoneNumberType,
}

/// Type of phone number
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PhoneNumberType {
    Mobile,
    Home,
    Work,
    Main,
    Other,
}

/// Represents an SMS/MMS message
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SmsMessage {
    /// Unique message ID
    pub id: String,
    /// Thread/conversation ID
    pub thread_id: String,
    /// Phone number of the other party
    pub address: String,
    /// Contact name if available
    pub contact_name: Option<String>,
    /// Message body
    pub body: String,
    /// Timestamp in milliseconds since epoch
    pub timestamp: u64,
    /// Whether this message was sent by us or received
    pub is_outgoing: bool,
    /// Read status
    pub is_read: bool,
    /// Message status
    pub status: SmsStatus,
    /// MMS attachments if any
    pub attachments: Vec<MmsAttachment>,
}

/// SMS message status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SmsStatus {
    Pending,
    Sent,
    Delivered,
    Failed,
    Received,
}

/// MMS attachment
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MmsAttachment {
    pub id: String,
    pub content_type: String,
    pub filename: Option<String>,
    /// Base64 encoded data for small attachments, or a reference ID for larger ones
    pub data: Option<String>,
}

/// Represents a conversation thread
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Conversation {
    /// Thread ID
    pub id: String,
    /// Phone number(s) in this conversation
    pub addresses: Vec<String>,
    /// Contact name(s) if available
    pub contact_names: Vec<String>,
    /// Last message preview
    pub last_message: Option<String>,
    /// Timestamp of last message
    pub last_timestamp: u64,
    /// Unread message count
    pub unread_count: u32,
}

/// Represents a phone call log entry
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CallLogEntry {
    /// Unique call ID
    pub id: String,
    /// Phone number
    pub number: String,
    /// Contact name if available
    pub contact_name: Option<String>,
    /// Call type
    pub call_type: CallType,
    /// Timestamp in milliseconds since epoch
    pub timestamp: u64,
    /// Duration in seconds
    pub duration: u32,
    /// Whether the call was read/seen
    pub is_read: bool,
}

/// Type of phone call
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CallType {
    Incoming,
    Outgoing,
    Missed,
    Rejected,
    Blocked,
    Voicemail,
}

/// Active/ongoing call state
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActiveCall {
    /// Phone number
    pub number: String,
    /// Contact name if available
    pub contact_name: Option<String>,
    /// Call state
    pub state: ActiveCallState,
    /// Duration in seconds (for ongoing calls)
    pub duration: u32,
    /// Whether this is an incoming or outgoing call
    pub is_incoming: bool,
}

/// State of an active call
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ActiveCallState {
    Ringing,
    Dialing,
    Connected,
    OnHold,
    Ended,
}

/// Telephony control messages sent between devices
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TelephonyMessage {
    /// Request to sync contacts
    ContactsSyncRequest,
    /// Response with contacts list
    ContactsSyncResponse { contacts: Vec<Contact> },
    /// Request to sync conversations
    ConversationsSyncRequest,
    /// Response with conversations list
    ConversationsSyncResponse { conversations: Vec<Conversation> },
    /// Request messages for a specific thread
    MessagesRequest { thread_id: String, limit: u32, before_timestamp: Option<u64> },
    /// Response with messages
    MessagesResponse { thread_id: String, messages: Vec<SmsMessage> },
    /// Send a new SMS
    SendSms { to: String, body: String },
    /// SMS send result
    SmsSendResult { success: bool, message_id: Option<String>, error: Option<String> },
    /// New incoming SMS notification
    NewSmsNotification { message: SmsMessage },
    /// Request call log
    CallLogRequest { limit: u32, before_timestamp: Option<u64> },
    /// Response with call log
    CallLogResponse { entries: Vec<CallLogEntry> },
    /// Initiate a phone call
    InitiateCall { number: String },
    /// Call action (answer, reject, hangup, mute, etc.)
    CallAction { action: CallAction },
    /// Active call state update
    ActiveCallUpdate { call: Option<ActiveCall> },
    /// Mark messages as read
    MarkMessagesRead { thread_id: String, message_ids: Vec<String> },
    /// Delete messages
    DeleteMessages { message_ids: Vec<String> },
    /// Delete conversation
    DeleteConversation { thread_id: String },
}

/// Actions that can be performed on an active call
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CallAction {
    Answer,
    Reject,
    HangUp,
    Mute,
    Unmute,
    Hold,
    Unhold,
    SendDtmf(char),
}

/// Telephony capabilities that the phone reports
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TelephonyCapabilities {
    pub can_send_sms: bool,
    pub can_make_calls: bool,
    pub can_access_contacts: bool,
    pub can_access_call_log: bool,
}
