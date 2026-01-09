use serde::{Deserialize, Serialize};
use std::time::SystemTime;

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
        data: Vec<u8>, // Using raw bytes for simplicity, could be optimized later
    },
    WriteFileRequest {
        path: String,
        offset: u64,
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
    CreateDirRequest {
        path: String,
    },
    DeleteRequest {
        path: String,
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
    // Add other methods as needed
}
