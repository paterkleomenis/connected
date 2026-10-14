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
/// newly created `Device`s. Current peers use postcard; v1 peers use raw JSON.
pub const PROTOCOL_VERSION: u32 = 3;
/// Oldest peer protocol version supported by the compatibility codec.
pub const MIN_COMPATIBLE_PROTOCOL_VERSION: u32 = 1;

/// Default UDP port for the QUIC transport. A stable port (instead of an
/// OS-assigned ephemeral one) lets users create persistent firewall rules —
/// e.g. `sudo ufw allow 47999/udp` — which survive app restarts. The actual
/// bound port is always advertised via mDNS, so peers never assume this value.
/// KEEP IN SYNC: android ConnectedApp.kt + ios ConnectedAppModel.swift
/// hardcode the same value (their UniFFI bindings are snapshotted).
pub const DEFAULT_QUIC_PORT: u16 = 47999;
/// How many consecutive ports past the requested one to try when the
/// requested port is already in use, before falling back to an ephemeral port.
pub const PORT_FALLBACK_ATTEMPTS: u16 = 10;
