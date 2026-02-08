use serde::{Deserialize, Serialize};

/// Custom serde module for encoding Vec<u8> as base64 strings in JSON.
/// This avoids the default JSON serialization of Vec<u8> as an array of numbers,
/// which causes ~4x size expansion and can easily exceed message size limits.
mod base64_bytes {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(data: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        let encoded = STANDARD.encode(data);
        encoded.serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(deserializer)?;
        STANDARD.decode(&s).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FsEntryType {
    File,
    Directory,
    Symlink,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FsEntry {
    pub name: String,
    pub path: String,
    pub entry_type: FsEntryType,
    pub size: u64,
    pub modified: Option<u64>, // Unix timestamp in seconds
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FilesystemMessage {
    ListDirRequest {
        path: String,
    },
    ListDirResponse {
        entries: Vec<FsEntry>,
    },
    ReadFileRequest {
        path: String,
        offset: u64,
        size: u64,
    },
    ReadFileResponse {
        #[serde(with = "base64_bytes")]
        data: Vec<u8>,
    },
    WriteFileRequest {
        path: String,
        offset: u64,
        #[serde(with = "base64_bytes")]
        data: Vec<u8>,
    },
    WriteFileResponse {
        bytes_written: u64,
    },
    GetMetadataRequest {
        path: String,
    },
    GetMetadataResponse {
        entry: FsEntry,
    },
    /// NOTE: CreateDirRequest is intentionally not handled by the protocol dispatcher.
    /// It exists as a protocol variant but is rejected at the handler level for security.
    /// Do NOT enable without strong authorization and user-confirmation flow.
    CreateDirRequest {
        path: String,
    },
    /// NOTE: DeleteRequest is intentionally not handled by the protocol dispatcher.
    /// It exists as a protocol variant but is rejected at the handler level for security.
    /// Do NOT enable without strong authorization and user-confirmation flow.
    DeleteRequest {
        path: String,
    },
    GetThumbnailRequest {
        path: String,
    },
    GetThumbnailResponse {
        #[serde(with = "base64_bytes")]
        data: Vec<u8>,
    },
    Error {
        message: String,
    },
    Ack,
}

// Stream type for Filesystem operations
pub const STREAM_TYPE_FS: u8 = 3;

pub trait FilesystemProvider: Send + Sync {
    fn list_dir(&self, path: &str) -> crate::error::Result<Vec<FsEntry>>;
    fn read_file(&self, path: &str, offset: u64, size: u64) -> crate::error::Result<Vec<u8>>;
    fn write_file(&self, path: &str, offset: u64, data: &[u8]) -> crate::error::Result<u64>;
    fn get_metadata(&self, path: &str) -> crate::error::Result<FsEntry>;

    fn get_thumbnail(&self, _path: &str) -> crate::error::Result<Vec<u8>> {
        Err(crate::error::ConnectedError::NotImplemented)
    }
}
