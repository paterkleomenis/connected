pub mod client;
pub mod device;
pub mod discovery;
pub mod error;
pub mod events;
pub mod file_transfer;
pub mod filesystem;
pub mod security;
pub mod telephony;
pub mod transport;
pub mod update;

pub use client::ConnectedClient;
pub use device::{Device, DeviceType};
pub use discovery::{DiscoveryEvent, DiscoveryService};
pub use error::{ConnectedError, Result};
pub use events::{ConnectedEvent, TransferDirection};
pub use file_transfer::{FileTransfer, FileTransferMessage, TransferProgress};
pub use filesystem::{FilesystemMessage, FsEntry, FsEntryType};
pub use security::{PeerInfo, PeerStatus};
pub use telephony::{
    ActiveCall, ActiveCallState, CallAction, CallLogEntry, CallType, Contact, Conversation,
    MmsAttachment, PhoneNumber, PhoneNumberType, SmsMessage, SmsStatus, TelephonyCapabilities,
    TelephonyMessage,
};
pub use transport::{
    MediaCommand, MediaControlMessage, MediaState, Message, QuicTransport, UnpairReason,
};
pub use update::{UpdateChecker, UpdateInfo, download_to_file};
