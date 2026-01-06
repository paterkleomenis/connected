use crate::device::Device;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConnectedEvent {
    /// A new device has been discovered on the network
    DeviceFound(Device),
    /// A device is no longer reachable
    DeviceLost(String),
    /// File transfer has started
    TransferStarting {
        id: String, // transfer_id
        filename: String,
        total_size: u64,
        peer_device: String,
        direction: TransferDirection,
    },
    /// File transfer progress update
    TransferProgress {
        id: String,
        bytes_transferred: u64,
        total_size: u64,
    },
    /// File transfer completed successfully
    TransferCompleted { id: String, filename: String },
    /// File transfer failed
    TransferFailed { id: String, error: String },
    /// Clipboard content received
    ClipboardReceived {
        content: String,
        from_device: String,
    },
    /// Critical error in a subsystem
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferDirection {
    Incoming,
    Outgoing,
}
