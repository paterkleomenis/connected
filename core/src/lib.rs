pub mod ios_paths;

pub mod client;
pub mod codec;
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
    MediaCommand, MediaControlMessage, MediaState, Message, QuicTransport, RemoteCommand,
    RemoteCommandMessage,
};
pub use update::{
    UpdateChecker, UpdateInfo, download_to_file, install_linux_appimage_update,
    install_macos_update,
};

/// Wire-protocol version advertised via mDNS TXT records and embedded in
/// newly created `Device`s. Single source of truth: bumping this constant is
/// the ONLY change needed to move the protocol version — `discovery.rs` uses
/// it for announce/compat checks and `device.rs` stamps it into new devices.
pub const PROTOCOL_VERSION: u32 = 2;
/// Oldest peer protocol version we still interoperate with.
pub const MIN_COMPATIBLE_PROTOCOL_VERSION: u32 = 1;
