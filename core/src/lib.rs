pub mod client;
pub mod device;
pub mod discovery;
pub mod error;
pub mod events;
pub mod file_transfer;
pub mod security;
pub mod transport;

pub use client::ConnectedClient;
pub use device::{Device, DeviceType};
pub use discovery::{DiscoveryEvent, DiscoveryService};
pub use error::{ConnectedError, Result};
pub use events::ConnectedEvent;
pub use file_transfer::{FileTransfer, FileTransferMessage, TransferProgress};
pub use transport::{Message, QuicTransport};
