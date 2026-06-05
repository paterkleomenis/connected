use crate::error::{ConnectedError, Result};
// Use transport constants if public, or redefine
use bytes::BytesMut;
use quinn::{Connection, RecvStream, SendStream};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

// We need to match transport constants
const STREAM_TYPE_FILE: u8 = 2;
// 2MB chunks keep throughput high while making cancellation noticeably more
// responsive than very large (16MB) writes on slower links.
const BUFFER_SIZE: usize = 2 * 1024 * 1024;
/// Timeout for receiving a data chunk during file transfer (30 seconds).
/// If no data is received within this window, the peer is considered disconnected.
const READ_CHUNK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// Maximum allowed incoming file size (100 GB). Transfers exceeding this are rejected.
const MAX_INCOMING_FILE_SIZE: u64 = 100 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FileTransferMessage {
    /// Request to send a file
    SendRequest {
        filename: String,
        size: u64,
        mime_type: Option<String>,
    },
    /// Accept file transfer
    Accept,
    /// Accept file transfer with resume offset
    AcceptResume { offset: u64 },
    /// Reject file transfer
    Reject { reason: String },
    /// Transfer complete (sent after raw data)
    Complete { checksum: String },
    /// Acknowledge completion
    Ack,
    /// Error during transfer
    Error { message: String },
    /// Cancel transfer
    Cancel,

    // --- Folder / Batch Transfer Extensions ---
    /// Request to send a folder structure or a flat list of files
    BatchRequest {
        batch_id: String,   // Unique ID for this batch transfer
        name: String,       // Directory name or "Batch Transfer"
        total_size: u64,    // Combined size of all files
        files_count: u64,   // Number of files to transfer
        is_directory: bool, // true for nested folder hierarchy, false for a flat batch
    },
    /// Declare a single item (directory or file) inside a batch/folder
    BatchItem {
        relative_path: String, // Relative path from the root
        is_dir: bool,
        size: u64,
    },
    /// Declare a single item stream inside a batch/folder (used concurrently on separate streams)
    BatchItemStream {
        batch_id: String,
        relative_path: String,
        size: u64,
    },
    /// Sent by the sender after completing the transmission of a single file in the batch
    ItemComplete { checksum: String },
    /// Sent by the receiver to acknowledge the successful receipt of a single file in the batch
    ItemAck,
}

#[derive(Debug, Clone)]
pub enum TransferProgress {
    /// File transfer request received, waiting for user approval
    Pending {
        filename: String,
        total_size: u64,
        mime_type: Option<String>,
    },
    /// Compression progress for folder transfers (emitted during ZIP archiving)
    CompressionProgress {
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
        total_size: u64,
    },
    Progress {
        bytes_transferred: u64,
        total_size: u64,
    },
    Completed {
        filename: String,
        total_size: u64,
    },
    Failed {
        error: String,
    },
    Cancelled,
}

pub struct FileTransfer {
    connection: Connection,
}

impl FileTransfer {
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }

    /// Send a file or folder to the connected peer using a new multiplexed stream
    pub async fn send_file<P: AsRef<Path>>(
        &self,
        file_path: P,
        progress_tx: Option<mpsc::UnboundedSender<TransferProgress>>,
        cancel_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<()> {
        let path = file_path.as_ref().to_path_buf();

        if path.is_dir() {
            let dir_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("folder")
                .to_string();

            info!("Preparing native folder transfer: {}", dir_name);

            // Collect all entries recursively
            let mut items = Vec::new();
            let walk_dir = walkdir::WalkDir::new(&path).follow_links(false);
            for entry in walk_dir.into_iter().filter_map(|e| e.ok()) {
                if entry.path_is_symlink() {
                    continue;
                }
                let relative_path = entry
                    .path()
                    .strip_prefix(path.parent().unwrap_or(&path))
                    .map_err(|e| ConnectedError::Io(std::io::Error::other(e)))?
                    .to_string_lossy()
                    .into_owned()
                    .replace('\\', "/");

                let is_dir = entry.path().is_dir();
                let size = if is_dir {
                    0
                } else {
                    entry
                        .metadata()
                        .map_err(|e| ConnectedError::Io(std::io::Error::other(e)))?
                        .len()
                };

                items.push((entry.path().to_path_buf(), relative_path, is_dir, size));
            }

            self.send_batch_items(dir_name, true, items, progress_tx, cancel_flag)
                .await
        } else {
            // Open and get file metadata
            let mut file = match File::open(&path).await {
                Ok(f) => f,
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::PermissionDenied {
                        error!(
                            "EPERM opening file for send: {:?}. \
                             On iOS: ensure set_ios_paths() was called before initialize() \
                             and the file is within the app sandbox. \
                             On Windows: check antivirus real-time scanning.",
                            path
                        );
                    }
                    return Err(ConnectedError::Io(e));
                }
            };

            let metadata = file.metadata().await.map_err(ConnectedError::Io)?;
            let file_size = metadata.len();

            let filename = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unnamed")
                .to_string();

            info!(
                "Starting single file transfer: {} ({} bytes)",
                filename, file_size
            );

            // Check for cancellation before starting
            if cancel_flag
                .as_ref()
                .is_some_and(|c| c.load(Ordering::Relaxed))
            {
                if let Some(ref tx) = progress_tx {
                    let _ = tx.send(TransferProgress::Cancelled);
                }
                return Err(ConnectedError::TransferFailed("Cancelled".to_string()));
            }

            // Notify progress
            if let Some(ref tx) = progress_tx {
                let _ = tx.send(TransferProgress::Starting {
                    filename: filename.clone(),
                    total_size: file_size,
                });
            }

            // Open bidirectional stream for file transfer
            let (mut send, mut recv) = self
                .connection
                .open_bi()
                .await
                .map_err(ConnectedError::QuicConnection)?;

            // Write Stream Type Prefix
            send.write_all(&[STREAM_TYPE_FILE])
                .await
                .map_err(ConnectedError::QuicWrite)?;

            // Send transfer request
            let request = FileTransferMessage::SendRequest {
                filename: filename.clone(),
                size: file_size,
                mime_type: mime_guess::from_path(&path).first().map(|m| m.to_string()),
            };

            send_message(&mut send, &request).await?;

            // Wait for accept/reject/resume
            let response: FileTransferMessage = recv_message(&mut recv).await?;

            let mut offset = 0;
            match response {
                FileTransferMessage::Accept => {
                    debug!("Transfer accepted, starting to stream from beginning");
                }
                FileTransferMessage::AcceptResume {
                    offset: resume_offset,
                } => {
                    debug!("Transfer accepted with resume: offset={}", resume_offset);
                    offset = resume_offset;
                }
                FileTransferMessage::Reject { reason } => {
                    warn!("Transfer rejected: {}", reason);
                    if let Some(ref tx) = progress_tx {
                        let _ = tx.send(TransferProgress::Failed {
                            error: format!("Rejected: {}", reason),
                        });
                    }
                    return Err(ConnectedError::TransferRejected(reason));
                }
                _ => {
                    return Err(ConnectedError::Protocol(
                        "Unexpected response to file transfer request".to_string(),
                    ));
                }
            }

            // Seek to offset if resuming
            if offset > 0 {
                use tokio::io::SeekFrom;
                file.seek(SeekFrom::Start(offset))
                    .await
                    .map_err(ConnectedError::Io)?;
            }

            // Send file data
            let mut hasher = blake3::Hasher::new();

            // Pre-hash already sent data if resuming, so the final checksum verifies the complete file
            if offset > 0 {
                let mut pre_file = File::open(&path).await.map_err(ConnectedError::Io)?;
                let mut buf = vec![0u8; BUFFER_SIZE];
                let mut pre_hashed = 0;
                while pre_hashed < offset {
                    let to_read = std::cmp::min((offset - pre_hashed) as usize, buf.len());
                    pre_file
                        .read_exact(&mut buf[..to_read])
                        .await
                        .map_err(ConnectedError::Io)?;
                    hasher.update(&buf[..to_read]);
                    pre_hashed += to_read as u64;
                }
            }

            let mut remaining = file_size - offset;
            let mut bytes_sent = offset;
            let mut last_progress_update = std::time::Instant::now();
            let mut buf = BytesMut::with_capacity(BUFFER_SIZE);

            while remaining > 0 {
                if cancel_flag
                    .as_ref()
                    .is_some_and(|c| c.load(Ordering::Relaxed))
                {
                    if let Some(ref tx) = progress_tx {
                        let _ = tx.send(TransferProgress::Cancelled);
                    }
                    return Err(ConnectedError::TransferFailed("Cancelled".to_string()));
                }

                buf.clear();
                let limit = std::cmp::min(remaining as usize, BUFFER_SIZE);
                buf.reserve(limit);

                let bytes_read = file.read_buf(&mut buf).await.map_err(ConnectedError::Io)?;
                if bytes_read == 0 {
                    break;
                }

                let chunk = buf.split().freeze();
                hasher.update(&chunk);

                send.write_all(&chunk)
                    .await
                    .map_err(ConnectedError::QuicWrite)?;

                bytes_sent += bytes_read as u64;
                remaining -= bytes_read as u64;

                if last_progress_update.elapsed().as_millis() > 200 {
                    if let Some(ref tx) = progress_tx {
                        let _ = tx.send(TransferProgress::Progress {
                            bytes_transferred: bytes_sent,
                            total_size: file_size,
                        });
                    }
                    last_progress_update = std::time::Instant::now();
                }
            }

            // Final progress update
            if let Some(ref tx) = progress_tx {
                let _ = tx.send(TransferProgress::Progress {
                    bytes_transferred: bytes_sent,
                    total_size: file_size,
                });
            }

            // Send completion with checksum
            let checksum = hasher.finalize().to_string();
            let complete = FileTransferMessage::Complete { checksum };
            send_message(&mut send, &complete).await?;

            // Wait for acknowledgment
            let ack: FileTransferMessage = recv_message(&mut recv).await?;

            match ack {
                FileTransferMessage::Ack => {
                    info!("File transfer completed successfully");
                    if let Some(ref tx) = progress_tx {
                        let _ = tx.send(TransferProgress::Completed {
                            filename: filename.clone(),
                            total_size: file_size,
                        });
                    }
                }
                FileTransferMessage::Error { message } => {
                    error!("Transfer error: {}", message);
                    if let Some(ref tx) = progress_tx {
                        let _ = tx.send(TransferProgress::Failed {
                            error: message.clone(),
                        });
                    }
                    return Err(ConnectedError::TransferFailed(message));
                }
                _ => {
                    return Err(ConnectedError::Protocol(
                        "Unexpected response after file transfer completion".to_string(),
                    ));
                }
            }

            send.finish()?;
            Ok(())
        }
    }

    /// Send a flat list of files (batch transfer) sequentially over a single stream
    pub async fn send_batch(
        &self,
        name: &str,
        file_paths: &[PathBuf],
        progress_tx: Option<mpsc::UnboundedSender<TransferProgress>>,
        cancel_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<()> {
        let mut items = Vec::new();
        for path in file_paths {
            if !path.exists() {
                return Err(ConnectedError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("File not found: {:?}", path),
                )));
            }
            let filename = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unnamed")
                .to_string();

            let is_dir = path.is_dir();
            let size = if is_dir {
                0
            } else {
                path.metadata().map_err(ConnectedError::Io)?.len()
            };
            items.push((path.clone(), filename, is_dir, size));
        }

        self.send_batch_items(name.to_string(), false, items, progress_tx, cancel_flag)
            .await
    }

    /// Helper method to transfer items in a batch or folder recursively over a single stream
    async fn send_batch_items(
        &self,
        name: String,
        is_directory: bool,
        items: Vec<(PathBuf, String, bool, u64)>,
        progress_tx: Option<mpsc::UnboundedSender<TransferProgress>>,
        cancel_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<()> {
        let batch_id = uuid::Uuid::new_v4().to_string();
        let total_size: u64 = items
            .iter()
            .map(|(_, _, is_dir, size)| if *is_dir { 0 } else { *size })
            .sum();
        let files_count = items.iter().filter(|(_, _, is_dir, _)| !*is_dir).count() as u64;

        // Partition items into directories, small files, and large files
        let mut dirs = Vec::new();
        let mut small_files = Vec::new();
        let mut large_files = Vec::new();

        for item in items {
            let (_, _, is_dir, size) = &item;
            if *is_dir {
                dirs.push(item);
            } else if *size >= 10 * 1024 * 1024 {
                large_files.push(item);
            } else {
                small_files.push(item);
            }
        }

        // Open bidirectional stream for transfer (Control Stream)
        let (mut send, mut recv) = self
            .connection
            .open_bi()
            .await
            .map_err(ConnectedError::QuicConnection)?;

        // Write Stream Type Prefix
        send.write_all(&[STREAM_TYPE_FILE])
            .await
            .map_err(ConnectedError::QuicWrite)?;

        // Send BatchRequest
        let request = FileTransferMessage::BatchRequest {
            batch_id: batch_id.clone(),
            name: name.clone(),
            total_size,
            files_count,
            is_directory,
        };
        send_message(&mut send, &request).await?;

        // Wait for accept/reject
        let response: FileTransferMessage = recv_message(&mut recv).await?;
        match response {
            FileTransferMessage::Accept => {
                debug!("Batch transfer accepted");
            }
            FileTransferMessage::Reject { reason } => {
                warn!("Batch transfer rejected: {}", reason);
                if let Some(ref tx) = progress_tx {
                    let _ = tx.send(TransferProgress::Failed {
                        error: format!("Rejected: {}", reason),
                    });
                }
                return Err(ConnectedError::TransferRejected(reason));
            }
            _ => {
                return Err(ConnectedError::Protocol(
                    "Unexpected response to batch transfer request".to_string(),
                ));
            }
        }

        if let Some(ref tx) = progress_tx {
            let _ = tx.send(TransferProgress::Starting {
                filename: name.clone(),
                total_size,
            });
        }

        let overall_bytes_transferred = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let last_progress_update = Arc::new(parking_lot::Mutex::new(std::time::Instant::now()));

        // Send directories sequentially on the control stream
        for (_abs_path, rel_path, is_dir, size) in dirs {
            if cancel_flag
                .as_ref()
                .is_some_and(|c| c.load(Ordering::Relaxed))
            {
                let _ = send_message(&mut send, &FileTransferMessage::Cancel).await;
                if let Some(ref tx) = progress_tx {
                    let _ = tx.send(TransferProgress::Cancelled);
                }
                return Err(ConnectedError::TransferFailed("Cancelled".to_string()));
            }

            let item_msg = FileTransferMessage::BatchItem {
                relative_path: rel_path.clone(),
                is_dir,
                size,
            };
            send_message(&mut send, &item_msg).await?;

            let item_resp: FileTransferMessage = recv_message(&mut recv).await?;
            match item_resp {
                FileTransferMessage::Accept
                | FileTransferMessage::Ack
                | FileTransferMessage::ItemAck => {
                    continue;
                }
                FileTransferMessage::Reject { reason } => {
                    return Err(ConnectedError::TransferFailed(format!(
                        "Failed to create directory {}: {}",
                        rel_path, reason
                    )));
                }
                _ => {
                    return Err(ConnectedError::Protocol(
                        "Unexpected response to directory item".to_string(),
                    ));
                }
            }
        }

        // Send small files sequentially on the control stream (pipelined)
        for (abs_path, rel_path, is_dir, size) in small_files {
            if cancel_flag
                .as_ref()
                .is_some_and(|c| c.load(Ordering::Relaxed))
            {
                let _ = send_message(&mut send, &FileTransferMessage::Cancel).await;
                if let Some(ref tx) = progress_tx {
                    let _ = tx.send(TransferProgress::Cancelled);
                }
                return Err(ConnectedError::TransferFailed("Cancelled".to_string()));
            }

            let item_msg = FileTransferMessage::BatchItem {
                relative_path: rel_path.clone(),
                is_dir,
                size,
            };
            send_message(&mut send, &item_msg).await?;

            let item_resp: FileTransferMessage = recv_message(&mut recv).await?;
            let offset = match item_resp {
                FileTransferMessage::Accept => 0,
                FileTransferMessage::AcceptResume {
                    offset: resume_offset,
                } => {
                    let total_sent = overall_bytes_transferred
                        .fetch_add(resume_offset, Ordering::SeqCst)
                        + resume_offset;
                    if let Some(ref tx) = progress_tx {
                        let mut last_update = last_progress_update.lock();
                        if last_update.elapsed().as_millis() > 200 {
                            let _ = tx.send(TransferProgress::Progress {
                                bytes_transferred: total_sent,
                                total_size,
                            });
                            *last_update = std::time::Instant::now();
                        }
                    }
                    resume_offset
                }
                FileTransferMessage::Reject { reason } => {
                    return Err(ConnectedError::TransferFailed(format!(
                        "File rejected: {}",
                        reason
                    )));
                }
                _ => {
                    return Err(ConnectedError::Protocol(
                        "Unexpected response to file item".to_string(),
                    ));
                }
            };

            if size == 0 || offset >= size {
                let checksum = if size > 0 {
                    let mut file = File::open(&abs_path).await.map_err(ConnectedError::Io)?;
                    let mut hasher = blake3::Hasher::new();
                    let mut buf = vec![0u8; BUFFER_SIZE];
                    loop {
                        let n = file.read(&mut buf).await.map_err(ConnectedError::Io)?;
                        if n == 0 {
                            break;
                        }
                        hasher.update(&buf[..n]);
                    }
                    hasher.finalize().to_string()
                } else {
                    blake3::hash(&[]).to_string()
                };

                send_message(&mut send, &FileTransferMessage::ItemComplete { checksum }).await?;
                match recv_message(&mut recv).await? {
                    FileTransferMessage::ItemAck => {}
                    FileTransferMessage::Error { message } => {
                        return Err(ConnectedError::TransferFailed(message));
                    }
                    _ => return Err(ConnectedError::Protocol("Expected ItemAck".to_string())),
                }
                continue;
            }

            let mut file = File::open(&abs_path).await.map_err(ConnectedError::Io)?;
            if offset > 0 {
                use tokio::io::SeekFrom;
                file.seek(SeekFrom::Start(offset))
                    .await
                    .map_err(ConnectedError::Io)?;
            }

            let mut remaining = size - offset;
            let mut hasher = blake3::Hasher::new();

            if offset > 0 {
                let mut pre_file = File::open(&abs_path).await.map_err(ConnectedError::Io)?;
                let mut buf = vec![0u8; BUFFER_SIZE];
                let mut pre_hashed = 0;
                while pre_hashed < offset {
                    let to_read = std::cmp::min((offset - pre_hashed) as usize, buf.len());
                    pre_file
                        .read_exact(&mut buf[..to_read])
                        .await
                        .map_err(ConnectedError::Io)?;
                    hasher.update(&buf[..to_read]);
                    pre_hashed += to_read as u64;
                }
            }

            let mut buf = BytesMut::with_capacity(BUFFER_SIZE);
            while remaining > 0 {
                if cancel_flag
                    .as_ref()
                    .is_some_and(|c| c.load(Ordering::Relaxed))
                {
                    let _ = send_message(&mut send, &FileTransferMessage::Cancel).await;
                    if let Some(ref tx) = progress_tx {
                        let _ = tx.send(TransferProgress::Cancelled);
                    }
                    return Err(ConnectedError::TransferFailed("Cancelled".to_string()));
                }

                buf.clear();
                let limit = std::cmp::min(remaining as usize, BUFFER_SIZE);
                buf.reserve(limit);

                let bytes_read = file.read_buf(&mut buf).await.map_err(ConnectedError::Io)?;
                if bytes_read == 0 {
                    break;
                }

                let chunk = buf.split().freeze();
                hasher.update(&chunk);

                send.write_all(&chunk)
                    .await
                    .map_err(ConnectedError::QuicWrite)?;

                let total_sent = overall_bytes_transferred
                    .fetch_add(bytes_read as u64, Ordering::SeqCst)
                    + (bytes_read as u64);
                if let Some(ref tx) = progress_tx {
                    let mut last_update = last_progress_update.lock();
                    if last_update.elapsed().as_millis() > 200 {
                        let _ = tx.send(TransferProgress::Progress {
                            bytes_transferred: total_sent,
                            total_size,
                        });
                        *last_update = std::time::Instant::now();
                    }
                }

                remaining -= bytes_read as u64;
            }

            let checksum = hasher.finalize().to_string();
            send_message(&mut send, &FileTransferMessage::ItemComplete { checksum }).await?;

            match recv_message(&mut recv).await? {
                FileTransferMessage::ItemAck => {}
                FileTransferMessage::Error { message } => {
                    return Err(ConnectedError::TransferFailed(message));
                }
                _ => return Err(ConnectedError::Protocol("Expected ItemAck".to_string())),
            }
        }

        // Stream large files concurrently using parallel sub-streams (limit to 3 concurrent transfers)
        let semaphore = Arc::new(tokio::sync::Semaphore::new(3));
        let mut join_set = tokio::task::JoinSet::new();

        for (abs_path, rel_path, _is_dir, size) in large_files {
            let connection = self.connection.clone();
            let batch_id = batch_id.clone();
            let cancel_flag = cancel_flag.clone();
            let semaphore = semaphore.clone();
            let overall_bytes_transferred = overall_bytes_transferred.clone();
            let last_progress_update = last_progress_update.clone();
            let progress_tx = progress_tx.clone();

            join_set.spawn(async move {
                let _permit = semaphore
                    .acquire_owned()
                    .await
                    .map_err(|_| ConnectedError::TransferFailed("Semaphore closed".to_string()))?;

                let (mut sub_send, mut sub_recv) = connection
                    .open_bi()
                    .await
                    .map_err(ConnectedError::QuicConnection)?;
                sub_send
                    .write_all(&[STREAM_TYPE_FILE])
                    .await
                    .map_err(ConnectedError::QuicWrite)?;

                let stream_msg = FileTransferMessage::BatchItemStream {
                    batch_id: batch_id.clone(),
                    relative_path: rel_path.clone(),
                    size,
                };
                send_message(&mut sub_send, &stream_msg).await?;

                let item_resp: FileTransferMessage = recv_message(&mut sub_recv).await?;
                let offset = match item_resp {
                    FileTransferMessage::Accept => 0,
                    FileTransferMessage::AcceptResume {
                        offset: resume_offset,
                    } => {
                        let total_sent = overall_bytes_transferred
                            .fetch_add(resume_offset, Ordering::SeqCst)
                            + resume_offset;
                        if let Some(ref tx) = progress_tx {
                            let mut last_update = last_progress_update.lock();
                            if last_update.elapsed().as_millis() > 200 {
                                let _ = tx.send(TransferProgress::Progress {
                                    bytes_transferred: total_sent,
                                    total_size,
                                });
                                *last_update = std::time::Instant::now();
                            }
                        }
                        resume_offset
                    }
                    FileTransferMessage::Reject { reason } => {
                        return Err(ConnectedError::TransferFailed(format!(
                            "File rejected: {}",
                            reason
                        )));
                    }
                    _ => {
                        return Err(ConnectedError::Protocol(
                            "Unexpected response to file item stream".to_string(),
                        ));
                    }
                };

                if size == 0 || offset >= size {
                    let checksum = if size > 0 {
                        let mut file = File::open(&abs_path).await.map_err(ConnectedError::Io)?;
                        let mut hasher = blake3::Hasher::new();
                        let mut buf = vec![0u8; BUFFER_SIZE];
                        loop {
                            let n = file.read(&mut buf).await.map_err(ConnectedError::Io)?;
                            if n == 0 {
                                break;
                            }
                            hasher.update(&buf[..n]);
                        }
                        hasher.finalize().to_string()
                    } else {
                        blake3::hash(&[]).to_string()
                    };

                    send_message(
                        &mut sub_send,
                        &FileTransferMessage::ItemComplete { checksum },
                    )
                    .await?;
                    match recv_message(&mut sub_recv).await? {
                        FileTransferMessage::ItemAck => {}
                        FileTransferMessage::Error { message } => {
                            return Err(ConnectedError::TransferFailed(message));
                        }
                        _ => return Err(ConnectedError::Protocol("Expected ItemAck".to_string())),
                    }
                    return Ok(());
                }

                let mut file = File::open(&abs_path).await.map_err(ConnectedError::Io)?;
                if offset > 0 {
                    use tokio::io::SeekFrom;
                    file.seek(SeekFrom::Start(offset))
                        .await
                        .map_err(ConnectedError::Io)?;
                }

                let mut remaining = size - offset;
                let mut hasher = blake3::Hasher::new();

                if offset > 0 {
                    let mut pre_file = File::open(&abs_path).await.map_err(ConnectedError::Io)?;
                    let mut buf = vec![0u8; BUFFER_SIZE];
                    let mut pre_hashed = 0;
                    while pre_hashed < offset {
                        let to_read = std::cmp::min((offset - pre_hashed) as usize, buf.len());
                        pre_file
                            .read_exact(&mut buf[..to_read])
                            .await
                            .map_err(ConnectedError::Io)?;
                        hasher.update(&buf[..to_read]);
                        pre_hashed += to_read as u64;
                    }
                }

                let mut buf = BytesMut::with_capacity(BUFFER_SIZE);
                while remaining > 0 {
                    if cancel_flag
                        .as_ref()
                        .is_some_and(|c| c.load(Ordering::Relaxed))
                    {
                        let _ = send_message(&mut sub_send, &FileTransferMessage::Cancel).await;
                        return Err(ConnectedError::TransferFailed("Cancelled".to_string()));
                    }

                    buf.clear();
                    let limit = std::cmp::min(remaining as usize, BUFFER_SIZE);
                    buf.reserve(limit);

                    let bytes_read = file.read_buf(&mut buf).await.map_err(ConnectedError::Io)?;
                    if bytes_read == 0 {
                        break;
                    }

                    let chunk = buf.split().freeze();
                    hasher.update(&chunk);

                    sub_send
                        .write_all(&chunk)
                        .await
                        .map_err(ConnectedError::QuicWrite)?;

                    let total_sent = overall_bytes_transferred
                        .fetch_add(bytes_read as u64, Ordering::SeqCst)
                        + (bytes_read as u64);
                    if let Some(ref tx) = progress_tx {
                        let mut last_update = last_progress_update.lock();
                        if last_update.elapsed().as_millis() > 200 {
                            let _ = tx.send(TransferProgress::Progress {
                                bytes_transferred: total_sent,
                                total_size,
                            });
                            *last_update = std::time::Instant::now();
                        }
                    }

                    remaining -= bytes_read as u64;
                }

                let checksum = hasher.finalize().to_string();
                send_message(
                    &mut sub_send,
                    &FileTransferMessage::ItemComplete { checksum },
                )
                .await?;

                match recv_message(&mut sub_recv).await? {
                    FileTransferMessage::ItemAck => {}
                    FileTransferMessage::Error { message } => {
                        return Err(ConnectedError::TransferFailed(message));
                    }
                    _ => return Err(ConnectedError::Protocol("Expected ItemAck".to_string())),
                }

                sub_send.finish()?;
                Ok(())
            });
        }

        let mut cancel_check_interval =
            tokio::time::interval(std::time::Duration::from_millis(100));
        loop {
            tokio::select! {
                res = join_set.join_next() => {
                    match res {
                        Some(Ok(Ok(()))) => {}
                        Some(Ok(Err(e))) => {
                            join_set.abort_all();
                            if let Some(ref tx) = progress_tx {
                                let _ = tx.send(TransferProgress::Failed { error: e.to_string() });
                            }
                            return Err(e);
                        }
                        Some(Err(join_err)) => {
                            join_set.abort_all();
                            let err = ConnectedError::TransferFailed(format!("Task join error: {}", join_err));
                            if let Some(ref tx) = progress_tx {
                                let _ = tx.send(TransferProgress::Failed { error: err.to_string() });
                            }
                            return Err(err);
                        }
                        None => {
                            break;
                        }
                    }
                }
                _ = cancel_check_interval.tick() => {
                    if cancel_flag.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
                        join_set.abort_all();
                        let _ = send_message(&mut send, &FileTransferMessage::Cancel).await;
                        if let Some(ref tx) = progress_tx {
                            let _ = tx.send(TransferProgress::Cancelled);
                        }
                        return Err(ConnectedError::TransferFailed("Cancelled".to_string()));
                    }
                }
            }
        }

        // Send folder/batch completion
        let complete = FileTransferMessage::Complete {
            checksum: "batch".to_string(),
        };
        send_message(&mut send, &complete).await?;

        match recv_message(&mut recv).await? {
            FileTransferMessage::Ack => {
                if let Some(ref tx) = progress_tx {
                    let _ = tx.send(TransferProgress::Completed {
                        filename: name.clone(),
                        total_size,
                    });
                }
            }
            FileTransferMessage::Error { message } => {
                return Err(ConnectedError::TransferFailed(message));
            }
            _ => return Err(ConnectedError::Protocol("Expected Ack".to_string())),
        }

        send.finish()?;
        Ok(())
    }

    /// Helper function to receive a single file item's payload sequentially over a stream
    async fn receive_file_payload(
        send: &mut SendStream,
        recv: &mut RecvStream,
        save_dir: &Path,
        relative_path: &str,
        size: u64,
        cancel_flag: &Option<Arc<std::sync::atomic::AtomicBool>>,
        _progress_tx: &Option<mpsc::UnboundedSender<TransferProgress>>,
    ) -> Result<()> {
        let item_path = save_dir.join(relative_path);
        if let Some(parent) = item_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(ConnectedError::Io)?;
        }

        let part_path = PathBuf::from(format!("{}.part", item_path.to_string_lossy()));
        let mut offset = 0;
        if let Ok(metadata) = tokio::fs::metadata(&part_path).await {
            let len = metadata.len();
            if len < size {
                offset = len;
            } else if len == size {
                offset = size;
            }
        }

        if offset == 0
            && tokio::fs::metadata(&item_path)
                .await
                .map(|m| m.len() == size)
                .unwrap_or(false)
        {
            offset = size;
        }

        let accept_item = if offset > 0 {
            FileTransferMessage::AcceptResume { offset }
        } else {
            FileTransferMessage::Accept
        };
        send_message(send, &accept_item).await?;

        if offset == size {
            // Skip streaming, wait for ItemComplete
            let checksum_msg =
                match tokio::time::timeout(READ_CHUNK_TIMEOUT, recv_message(recv)).await {
                    Ok(Ok(FileTransferMessage::ItemComplete { checksum })) => checksum,
                    _ => {
                        return Err(ConnectedError::Protocol(
                            "Expected ItemComplete".to_string(),
                        ));
                    }
                };

            let our_checksum = if size > 0 {
                let path_to_hash = if part_path.exists() {
                    &part_path
                } else {
                    &item_path
                };
                let mut f = File::open(path_to_hash).await.map_err(ConnectedError::Io)?;
                let mut hasher = blake3::Hasher::new();
                let mut buf = vec![0u8; BUFFER_SIZE];
                loop {
                    let n = f.read(&mut buf).await.map_err(ConnectedError::Io)?;
                    if n == 0 {
                        break;
                    }
                    hasher.update(&buf[..n]);
                }
                hasher.finalize().to_string()
            } else {
                blake3::hash(&[]).to_string()
            };

            if our_checksum != checksum_msg {
                let error = FileTransferMessage::Error {
                    message: "Checksum mismatch".to_string(),
                };
                send_message(send, &error).await?;
                return Err(ConnectedError::ChecksumMismatch);
            }

            if part_path.exists() {
                tokio::fs::rename(&part_path, &item_path)
                    .await
                    .map_err(ConnectedError::Io)?;
            }

            send_message(send, &FileTransferMessage::ItemAck).await?;
            return Ok(());
        }

        let raw_file = if offset > 0 {
            tokio::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(&part_path)
                .await
                .map_err(ConnectedError::Io)?
        } else {
            File::create(&part_path).await.map_err(ConnectedError::Io)?
        };
        let mut file = tokio::io::BufWriter::with_capacity(BUFFER_SIZE, raw_file);

        if offset > 0 {
            use tokio::io::SeekFrom;
            file.seek(SeekFrom::Start(offset))
                .await
                .map_err(ConnectedError::Io)?;
        }

        let mut remaining = size - offset;
        let mut hasher = blake3::Hasher::new();

        if offset > 0 {
            let mut pre_file = File::open(&part_path).await.map_err(ConnectedError::Io)?;
            let mut buf = vec![0u8; BUFFER_SIZE];
            let mut pre_hashed = 0;
            while pre_hashed < offset {
                let to_read = std::cmp::min((offset - pre_hashed) as usize, buf.len());
                pre_file
                    .read_exact(&mut buf[..to_read])
                    .await
                    .map_err(ConnectedError::Io)?;
                hasher.update(&buf[..to_read]);
                pre_hashed += to_read as u64;
            }
        }

        while remaining > 0 {
            if cancel_flag
                .as_ref()
                .is_some_and(|c| c.load(Ordering::Relaxed))
            {
                let _ = send_message(send, &FileTransferMessage::Cancel).await;
                return Err(ConnectedError::TransferFailed("Cancelled".to_string()));
            }

            let max_len = std::cmp::min(remaining, BUFFER_SIZE as u64) as usize;
            let chunk = match tokio::time::timeout(
                READ_CHUNK_TIMEOUT,
                recv.read_chunk(max_len, true),
            )
            .await
            {
                Ok(Ok(Some(chunk))) => chunk,
                _ => {
                    return Err(ConnectedError::Io(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "timed out receiving item data",
                    )));
                }
            };

            let bytes = chunk.bytes;
            let n = bytes.len() as u64;
            hasher.update(&bytes);
            file.write_all(&bytes).await.map_err(ConnectedError::Io)?;

            remaining -= n;
        }

        file.flush().await.map_err(ConnectedError::Io)?;
        file.into_inner()
            .sync_all()
            .await
            .map_err(ConnectedError::Io)?;

        // Read ItemComplete
        let complete_msg: FileTransferMessage =
            match tokio::time::timeout(READ_CHUNK_TIMEOUT, recv_message(recv)).await {
                Ok(Ok(msg)) => msg,
                _ => {
                    return Err(ConnectedError::Io(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "timed out waiting for ItemComplete",
                    )));
                }
            };

        match complete_msg {
            FileTransferMessage::ItemComplete { checksum } => {
                let our_checksum = hasher.finalize().to_string();
                if our_checksum != checksum {
                    error!(
                        "Checksum mismatch for item: expected {}, got {}",
                        checksum, our_checksum
                    );
                    let error = FileTransferMessage::Error {
                        message: "Checksum mismatch".to_string(),
                    };
                    send_message(send, &error).await?;
                    return Err(ConnectedError::ChecksumMismatch);
                }

                tokio::fs::rename(&part_path, &item_path)
                    .await
                    .map_err(ConnectedError::Io)?;
                send_message(send, &FileTransferMessage::ItemAck).await?;
                Ok(())
            }
            _ => Err(ConnectedError::Protocol(
                "Expected ItemComplete".to_string(),
            )),
        }
    }

    /// Receive a file transfer on an existing stream
    /// This version reads the request, waits for accept/reject, then receives the file
    /// Receive a file transfer on an existing stream
    /// This version reads the request, waits for accept/reject, then receives the file
    #[allow(clippy::too_many_arguments)]
    pub async fn handle_incoming(
        mut send: SendStream,
        mut recv: RecvStream,
        save_dir: impl AsRef<Path>,
        progress_tx: Option<mpsc::UnboundedSender<TransferProgress>>,
        auto_accept: bool,
        accept_rx: Option<tokio::sync::oneshot::Receiver<bool>>,
        cancel_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
        approved_batches: Option<
            Arc<parking_lot::RwLock<std::collections::HashMap<String, PathBuf>>>,
        >,
    ) -> Result<String> {
        // Read Request
        let request: FileTransferMessage = recv_message(&mut recv).await?;

        match request {
            FileTransferMessage::BatchItemStream {
                batch_id,
                relative_path,
                size,
            } => {
                info!(
                    "Incoming concurrent batch item: {} ({} bytes) for batch {}",
                    relative_path, size, batch_id
                );

                let batch_save_dir = if let Some(ref approved) = approved_batches {
                    let lock = approved.read();
                    lock.get(&batch_id).cloned()
                } else {
                    None
                };

                let Some(save_dir) = batch_save_dir else {
                    let reject = FileTransferMessage::Reject {
                        reason: "Batch not approved or active".to_string(),
                    };
                    send_message(&mut send, &reject).await?;
                    return Err(ConnectedError::TransferRejected(
                        "Batch not approved".to_string(),
                    ));
                };

                if !is_safe_relative_path(&relative_path) {
                    let err_msg =
                        format!("Directory traversal attempt detected: {}", relative_path);
                    error!("{}", err_msg);
                    let reject_msg = FileTransferMessage::Reject {
                        reason: "Security violation".to_string(),
                    };
                    send_message(&mut send, &reject_msg).await?;
                    return Err(ConnectedError::Protocol(err_msg));
                }

                Self::receive_file_payload(
                    &mut send,
                    &mut recv,
                    &save_dir,
                    &relative_path,
                    size,
                    &cancel_flag,
                    &progress_tx,
                )
                .await?;

                let item_path = save_dir.join(&relative_path);
                Ok(item_path.to_string_lossy().to_string())
            }
            FileTransferMessage::SendRequest {
                filename,
                size,
                mime_type: _mime_type,
            } => {
                info!(
                    "Incoming file transfer request: {} ({} bytes)",
                    filename, size
                );

                // M2: Reject transfers that exceed the maximum allowed size to prevent disk exhaustion.
                if size > MAX_INCOMING_FILE_SIZE {
                    let reject = FileTransferMessage::Reject {
                        reason: format!(
                            "File too large: {} bytes exceeds maximum allowed {} bytes",
                            size, MAX_INCOMING_FILE_SIZE
                        ),
                    };
                    let _ = send_message(&mut send, &reject).await;
                    if let Some(ref tx) = progress_tx {
                        let _ = tx.send(TransferProgress::Failed {
                            error: format!("File too large ({} bytes)", size),
                        });
                    }
                    return Err(ConnectedError::TransferRejected(format!(
                        "File size {} exceeds maximum allowed {}",
                        size, MAX_INCOMING_FILE_SIZE
                    )));
                }

                // If we need user approval, emit Pending first
                if !auto_accept && let Some(ref tx) = progress_tx {
                    let _ = tx.send(TransferProgress::Pending {
                        filename: filename.clone(),
                        total_size: size,
                        mime_type: _mime_type.clone(),
                    });
                }

                // Accept or reject based on auto_accept or user decision
                let should_accept = if auto_accept {
                    true
                } else if let Some(rx) = accept_rx {
                    // Wait for user decision with a timeout
                    match tokio::time::timeout(std::time::Duration::from_secs(60), rx).await {
                        Ok(Ok(accepted)) => accepted,
                        _ => false, // Channel closed or timeout, treat as rejection
                    }
                } else {
                    false
                };

                if !should_accept {
                    let reject = FileTransferMessage::Reject {
                        reason: "User declined".to_string(),
                    };
                    send_message(&mut send, &reject).await?;
                    if let Some(ref tx) = progress_tx {
                        let _ = tx.send(TransferProgress::Cancelled);
                    }
                    return Err(ConnectedError::TransferRejected(
                        "User declined".to_string(),
                    ));
                }

                // Sanitize filename and avoid overwriting existing files
                let mut safe_filename = sanitize_filename(&filename);
                let save_dir = save_dir.as_ref();
                let mut save_path = save_dir.join(&safe_filename);

                // If the target exists, generate a unique name: "name (1).ext"
                let mut exists = match tokio::fs::metadata(&save_path).await {
                    Ok(_) => true,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
                    Err(e) => {
                        let err = ConnectedError::Io(e);
                        let _ = send_message(
                            &mut send,
                            &FileTransferMessage::Reject {
                                reason: "Failed to prepare file".to_string(),
                            },
                        )
                        .await;
                        if let Some(ref tx) = progress_tx {
                            let _ = tx.send(TransferProgress::Failed {
                                error: err.to_string(),
                            });
                        }
                        return Err(err);
                    }
                };
                if exists {
                    let path = Path::new(&safe_filename);
                    let mut stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("file")
                        .to_string();
                    if stem.is_empty() {
                        stem = "file".to_string();
                    }
                    let ext = path
                        .extension()
                        .and_then(|s| s.to_str())
                        .map(|s| s.to_string());

                    for i in 1..=10_000 {
                        let candidate = match &ext {
                            Some(ext) => format!("{} ({}).{}", stem, i, ext),
                            None => format!("{} ({})", stem, i),
                        };
                        let candidate_path = save_dir.join(&candidate);
                        match tokio::fs::metadata(&candidate_path).await {
                            Ok(_) => {}
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                                save_path = candidate_path;
                                safe_filename = candidate;
                                exists = false;
                                break;
                            }
                            Err(e) => {
                                let err = ConnectedError::Io(e);
                                let _ = send_message(
                                    &mut send,
                                    &FileTransferMessage::Reject {
                                        reason: "Failed to prepare file".to_string(),
                                    },
                                )
                                .await;
                                if let Some(ref tx) = progress_tx {
                                    let _ = tx.send(TransferProgress::Failed {
                                        error: err.to_string(),
                                    });
                                }
                                return Err(err);
                            }
                        }
                    }

                    if exists {
                        let err = ConnectedError::Io(std::io::Error::new(
                            std::io::ErrorKind::AlreadyExists,
                            "unable to pick unique filename",
                        ));
                        let _ = send_message(
                            &mut send,
                            &FileTransferMessage::Reject {
                                reason: "Failed to prepare file".to_string(),
                            },
                        )
                        .await;
                        if let Some(ref tx) = progress_tx {
                            let _ = tx.send(TransferProgress::Failed {
                                error: err.to_string(),
                            });
                        }
                        return Err(err);
                    }
                }

                // Check if we can resume from a partial .part file
                let part_path = PathBuf::from(format!("{}.part", save_path.to_string_lossy()));
                let mut offset = 0;
                if let Ok(metadata) = tokio::fs::metadata(&part_path).await {
                    let len = metadata.len();
                    if len < size {
                        offset = len;
                    }
                }

                let accept_msg = if offset > 0 {
                    FileTransferMessage::AcceptResume { offset }
                } else {
                    FileTransferMessage::Accept
                };

                send_message(&mut send, &accept_msg).await?;

                // Now notify that transfer is starting
                if let Some(ref tx) = progress_tx {
                    let _ = tx.send(TransferProgress::Starting {
                        filename: safe_filename.clone(),
                        total_size: size,
                    });
                }

                // Create or open the partial file
                let raw_file = if offset > 0 {
                    tokio::fs::OpenOptions::new()
                        .write(true)
                        .create(true)
                        .truncate(false)
                        .open(&part_path)
                        .await
                        .map_err(ConnectedError::Io)?
                } else {
                    File::create(&part_path).await.map_err(ConnectedError::Io)?
                };
                let mut file = tokio::io::BufWriter::with_capacity(BUFFER_SIZE, raw_file);

                if offset > 0 {
                    use tokio::io::SeekFrom;
                    file.seek(SeekFrom::Start(offset))
                        .await
                        .map_err(ConnectedError::Io)?;
                }

                let mut bytes_received = offset;
                let mut hasher = blake3::Hasher::new();
                let mut last_progress_update = std::time::Instant::now();

                // Pre-hash already received bytes if resuming
                if offset > 0 {
                    let mut pre_file = File::open(&part_path).await.map_err(ConnectedError::Io)?;
                    let mut buf = vec![0u8; BUFFER_SIZE];
                    let mut pre_hashed = 0;
                    while pre_hashed < offset {
                        let to_read = std::cmp::min((offset - pre_hashed) as usize, buf.len());
                        pre_file
                            .read_exact(&mut buf[..to_read])
                            .await
                            .map_err(ConnectedError::Io)?;
                        hasher.update(&buf[..to_read]);
                        pre_hashed += to_read as u64;
                    }
                }

                let mut remaining = size - offset;
                while remaining > 0 {
                    if cancel_flag
                        .as_ref()
                        .is_some_and(|c| c.load(Ordering::Relaxed))
                    {
                        let _ = send_message(&mut send, &FileTransferMessage::Cancel).await;
                        if let Some(ref tx) = progress_tx {
                            let _ = tx.send(TransferProgress::Cancelled);
                        }
                        return Err(ConnectedError::TransferFailed(
                            "Cancelled by receiver".to_string(),
                        ));
                    }

                    let max_len = std::cmp::min(remaining, BUFFER_SIZE as u64) as usize;
                    let chunk = match tokio::time::timeout(
                        READ_CHUNK_TIMEOUT,
                        recv.read_chunk(max_len, true),
                    )
                    .await
                    {
                        Ok(Ok(Some(chunk))) => chunk,
                        Ok(Ok(None)) => {
                            return Err(ConnectedError::Io(std::io::Error::new(
                                std::io::ErrorKind::UnexpectedEof,
                                "unexpected EOF while receiving file data",
                            )));
                        }
                        Ok(Err(e)) => {
                            return Err(ConnectedError::Io(std::io::Error::new(
                                std::io::ErrorKind::ConnectionAborted,
                                format!("connection error while receiving file data: {}", e),
                            )));
                        }
                        Err(_) => {
                            return Err(ConnectedError::Io(std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "read timed out while receiving file data - peer disconnected",
                            )));
                        }
                    };

                    let bytes = chunk.bytes;
                    let n = bytes.len() as u64;
                    hasher.update(&bytes);
                    file.write_all(&bytes).await.map_err(ConnectedError::Io)?;

                    bytes_received += n;
                    remaining -= n;

                    if last_progress_update.elapsed().as_millis() > 200 {
                        if let Some(ref tx) = progress_tx {
                            let _ = tx.send(TransferProgress::Progress {
                                bytes_transferred: bytes_received,
                                total_size: size,
                            });
                        }
                        last_progress_update = std::time::Instant::now();
                    }
                }

                file.flush().await.map_err(ConnectedError::Io)?;
                file.into_inner()
                    .sync_all()
                    .await
                    .map_err(ConnectedError::Io)?;

                // Read Completion Message
                let complete: FileTransferMessage =
                    match tokio::time::timeout(READ_CHUNK_TIMEOUT, recv_message(&mut recv)).await {
                        Ok(Ok(msg)) => msg,
                        _ => {
                            return Err(ConnectedError::Io(std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "timed out waiting for completion message - peer disconnected",
                            )));
                        }
                    };

                match complete {
                    FileTransferMessage::Complete { checksum } => {
                        let our_checksum = hasher.finalize().to_string();
                        if our_checksum != checksum {
                            error!(
                                "Checksum mismatch: expected {}, got {}",
                                checksum, our_checksum
                            );
                            let error = FileTransferMessage::Error {
                                message: "Checksum mismatch".to_string(),
                            };
                            send_message(&mut send, &error).await?;
                            return Err(ConnectedError::ChecksumMismatch);
                        }

                        // Rename the partial file to final path
                        tokio::fs::rename(&part_path, &save_path)
                            .await
                            .map_err(ConnectedError::Io)?;

                        let ack = FileTransferMessage::Ack;
                        send_message(&mut send, &ack).await?;

                        if let Some(ref tx) = progress_tx {
                            let _ = tx.send(TransferProgress::Completed {
                                filename: safe_filename.clone(),
                                total_size: size,
                            });
                        }

                        Ok(save_path.to_string_lossy().to_string())
                    }
                    FileTransferMessage::Cancel => {
                        if let Some(ref tx) = progress_tx {
                            let _ = tx.send(TransferProgress::Cancelled);
                        }
                        Err(ConnectedError::TransferFailed(
                            "Transfer cancelled by sender".to_string(),
                        ))
                    }
                    FileTransferMessage::Error { message } => {
                        if let Some(ref tx) = progress_tx {
                            let _ = tx.send(TransferProgress::Failed {
                                error: message.clone(),
                            });
                        }
                        Err(ConnectedError::TransferFailed(message))
                    }
                    _ => Err(ConnectedError::Protocol(
                        "Expected Complete message".to_string(),
                    )),
                }
            }
            FileTransferMessage::BatchRequest {
                batch_id,
                name,
                total_size,
                files_count,
                is_directory,
            } => {
                info!(
                    "Incoming batch transfer: {} ({} files, {} bytes, is_dir={})",
                    name, files_count, total_size, is_directory
                );

                if total_size > MAX_INCOMING_FILE_SIZE {
                    let reject = FileTransferMessage::Reject {
                        reason: format!(
                            "Batch too large: {} bytes exceeds maximum allowed {} bytes",
                            total_size, MAX_INCOMING_FILE_SIZE
                        ),
                    };
                    let _ = send_message(&mut send, &reject).await;
                    if let Some(ref tx) = progress_tx {
                        let _ = tx.send(TransferProgress::Failed {
                            error: format!("Batch too large ({} bytes)", total_size),
                        });
                    }
                    return Err(ConnectedError::TransferRejected(
                        "Batch too large".to_string(),
                    ));
                }

                // If we need user approval, emit Pending first
                if !auto_accept && let Some(ref tx) = progress_tx {
                    let _ = tx.send(TransferProgress::Pending {
                        filename: name.clone(),
                        total_size,
                        mime_type: None,
                    });
                }

                let should_accept = if auto_accept {
                    true
                } else if let Some(rx) = accept_rx {
                    match tokio::time::timeout(std::time::Duration::from_secs(60), rx).await {
                        Ok(Ok(accepted)) => accepted,
                        _ => false,
                    }
                } else {
                    false
                };

                if !should_accept {
                    let reject = FileTransferMessage::Reject {
                        reason: "User declined".to_string(),
                    };
                    send_message(&mut send, &reject).await?;
                    if let Some(ref tx) = progress_tx {
                        let _ = tx.send(TransferProgress::Cancelled);
                    }
                    return Err(ConnectedError::TransferRejected(
                        "User declined".to_string(),
                    ));
                }

                if let Some(ref approved) = approved_batches {
                    approved
                        .write()
                        .insert(batch_id.clone(), save_dir.as_ref().to_path_buf());
                }

                struct ApprovedBatchGuard {
                    batch_id: String,
                    approved_batches: Option<
                        Arc<parking_lot::RwLock<std::collections::HashMap<String, PathBuf>>>,
                    >,
                }
                impl Drop for ApprovedBatchGuard {
                    fn drop(&mut self) {
                        if let Some(ref approved) = self.approved_batches {
                            approved.write().remove(&self.batch_id);
                        }
                    }
                }

                let _guard = ApprovedBatchGuard {
                    batch_id: batch_id.clone(),
                    approved_batches: approved_batches.clone(),
                };

                send_message(&mut send, &FileTransferMessage::Accept).await?;

                if let Some(ref tx) = progress_tx {
                    let _ = tx.send(TransferProgress::Starting {
                        filename: name.clone(),
                        total_size,
                    });
                }

                let save_dir = save_dir.as_ref();

                loop {
                    if cancel_flag
                        .as_ref()
                        .is_some_and(|c| c.load(Ordering::Relaxed))
                    {
                        let _ = send_message(&mut send, &FileTransferMessage::Cancel).await;
                        return Err(ConnectedError::TransferFailed(
                            "Cancelled by receiver".to_string(),
                        ));
                    }

                    let item_msg: FileTransferMessage =
                        match tokio::time::timeout(READ_CHUNK_TIMEOUT, recv_message(&mut recv))
                            .await
                        {
                            Ok(Ok(msg)) => msg,
                            _ => {
                                return Err(ConnectedError::Io(std::io::Error::new(
                                    std::io::ErrorKind::TimedOut,
                                    "timed out waiting for batch item",
                                )));
                            }
                        };

                    match item_msg {
                        FileTransferMessage::BatchItem {
                            relative_path,
                            is_dir,
                            size,
                        } => {
                            if !is_safe_relative_path(&relative_path) {
                                let err_msg = format!(
                                    "Directory traversal attempt detected: {}",
                                    relative_path
                                );
                                error!("{}", err_msg);
                                let reject_msg = FileTransferMessage::Reject {
                                    reason: "Security violation".to_string(),
                                };
                                send_message(&mut send, &reject_msg).await?;
                                return Err(ConnectedError::Protocol(err_msg));
                            }

                            if is_dir {
                                let item_path = save_dir.join(&relative_path);
                                tokio::fs::create_dir_all(&item_path)
                                    .await
                                    .map_err(ConnectedError::Io)?;
                                send_message(&mut send, &FileTransferMessage::ItemAck).await?;
                            } else {
                                Self::receive_file_payload(
                                    &mut send,
                                    &mut recv,
                                    save_dir,
                                    &relative_path,
                                    size,
                                    &cancel_flag,
                                    &progress_tx,
                                )
                                .await?;
                            }
                        }
                        FileTransferMessage::Complete { .. } => {
                            send_message(&mut send, &FileTransferMessage::Ack).await?;
                            if let Some(ref tx) = progress_tx {
                                let _ = tx.send(TransferProgress::Completed {
                                    filename: name.clone(),
                                    total_size,
                                });
                            }
                            return Ok(save_dir.to_string_lossy().to_string());
                        }
                        FileTransferMessage::Cancel => {
                            if let Some(ref tx) = progress_tx {
                                let _ = tx.send(TransferProgress::Cancelled);
                            }
                            return Err(ConnectedError::TransferFailed(
                                "Cancelled by sender".to_string(),
                            ));
                        }
                        _ => {
                            return Err(ConnectedError::Protocol(
                                "Unexpected message in batch transfer loop".to_string(),
                            ));
                        }
                    }
                }
            }
            _ => Err(ConnectedError::Protocol(
                "Expected SendRequest or BatchRequest".to_string(),
            )),
        }
    }
}

/// Helper function to validate path relative to the download directory to prevent directory traversal
pub fn is_safe_relative_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    if normalized.starts_with('/') {
        return false;
    }
    for segment in normalized.split('/') {
        if segment == ".." {
            return false;
        }
        if segment.contains(':') {
            return false;
        }
    }
    true
}

/// Send a message over the stream
pub(crate) async fn send_message<T: Serialize>(stream: &mut SendStream, message: &T) -> Result<()> {
    let data = serde_json::to_vec(message)?;
    let len: u32 = data.len().try_into().map_err(|_| {
        ConnectedError::Protocol(format!(
            "Message too large to send: {} bytes exceeds u32::MAX",
            data.len()
        ))
    })?;
    stream.write_all(&len.to_be_bytes()).await?;
    stream.write_all(&data).await?;
    Ok(())
}

/// Receive a message from the stream (default 4 MB limit for control messages)
pub(crate) async fn recv_message<T: for<'de> Deserialize<'de>>(
    stream: &mut RecvStream,
) -> Result<T> {
    recv_message_with_limit(stream, 4 * 1024 * 1024).await
}

/// Receive a message from the stream with a custom size limit.
/// Use this for filesystem messages where base64-encoded file data may exceed the
/// default 4 MB control-message limit.
///
/// Reads in fixed-size chunks rather than allocating the full declared length
/// up-front, so a malicious peer claiming a huge message length cannot force an
/// immediate multi-gigabyte allocation (DoS via memory exhaustion).
pub(crate) async fn recv_message_with_limit<T: for<'de> Deserialize<'de>>(
    stream: &mut RecvStream,
    max_size: usize,
) -> Result<T> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;

    if len == 0 {
        return Err(ConnectedError::Protocol(
            "Message length is zero".to_string(),
        ));
    }

    if len > max_size {
        return Err(ConnectedError::Protocol(format!(
            "Message too large: {} bytes (max {} bytes)",
            len, max_size
        )));
    }

    // Read in fixed-size chunks instead of allocating the full declared length
    // up-front. This bounds peak memory usage to the actual data received so far
    // rather than trusting the (potentially malicious) declared length.
    const READ_CHUNK_SIZE: usize = 64 * 1024; // 64 KB per chunk
    let mut data = Vec::with_capacity(len);
    let mut remaining = len;

    while remaining > 0 {
        let to_read = std::cmp::min(remaining, READ_CHUNK_SIZE);
        let prev_len = data.len();
        // Extend vector to hold the data
        data.resize(prev_len + to_read, 0);
        stream
            .read_exact(&mut data[prev_len..prev_len + to_read])
            .await?;
        remaining -= to_read;
    }

    let message = serde_json::from_slice(&data)?;
    Ok(message)
}

pub fn sanitize_filename(filename: &str) -> String {
    let name = Path::new(filename)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unnamed");

    let sanitized: String = name
        .chars()
        .filter(|c| {
            !matches!(
                c,
                '/' | '\\' | '\0' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
            )
        })
        .take(255)
        .collect();

    // Strip leading dots to prevent creating hidden files (e.g. `.bashrc`,
    // `.ssh`) that could silently overwrite important dot-files on Unix.
    let sanitized = sanitized.trim_start_matches('.').to_string();

    // Normalize common temporary-name artifacts added by mobile providers.
    // Examples:
    // - "file.pdf-abc123ef" (Android temp UUID suffix)
    // - "file.pdf-1712487000123" (timestamp suffix)
    // - "1712487000123_file.pdf" (timestamp prefix)
    let sanitized = strip_temp_prefix(&strip_temp_suffix(&sanitized));

    if sanitized.is_empty() {
        "unnamed".to_string()
    } else {
        sanitized
    }
}

fn strip_temp_suffix(filename: &str) -> String {
    let Some((base, suffix)) = filename.rsplit_once('-') else {
        return filename.to_string();
    };

    if base.is_empty() {
        return filename.to_string();
    }

    // Existing Android behavior: 8 hex chars appended after '-'.
    let is_hex_uuid_fragment = suffix.len() == 8 && suffix.chars().all(|c| c.is_ascii_hexdigit());

    // iOS/URI providers often append a unix timestamp (seconds or millis).
    // Only strip this variant when the base keeps a plausible extension.
    let is_timestamp_fragment = (10..=17).contains(&suffix.len())
        && suffix.chars().all(|c| c.is_ascii_digit())
        && has_plausible_extension(base);

    if is_hex_uuid_fragment || is_timestamp_fragment {
        base.to_string()
    } else {
        filename.to_string()
    }
}

fn strip_temp_prefix(filename: &str) -> String {
    let Some(pos) = filename.find(|c| ['-', '_'].contains(&c)) else {
        return filename.to_string();
    };

    let prefix = &filename[..pos];
    let rest = &filename[pos + 1..];

    let is_timestamp_prefix = (10..=17).contains(&prefix.len())
        && prefix.chars().all(|c| c.is_ascii_digit())
        && !rest.is_empty()
        && has_plausible_extension(rest);

    if is_timestamp_prefix {
        rest.to_string()
    } else {
        filename.to_string()
    }
}

fn has_plausible_extension(filename: &str) -> bool {
    Path::new(filename)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            !ext.is_empty() && ext.len() <= 10 && ext.chars().all(|c| c.is_ascii_alphanumeric())
        })
}

#[cfg(test)]
mod tests {
    use super::sanitize_filename;

    #[test]
    fn strips_android_uuid_suffix() {
        assert_eq!(sanitize_filename("file.pdf-abc123ef"), "file.pdf");
    }

    #[test]
    fn strips_timestamp_suffix_after_extension() {
        assert_eq!(sanitize_filename("report.pdf-1712487000123"), "report.pdf");
    }

    #[test]
    fn strips_timestamp_prefix_before_filename() {
        assert_eq!(sanitize_filename("1712487000123_report.pdf"), "report.pdf");
    }

    #[test]
    fn preserves_legitimate_numeric_filename_parts() {
        assert_eq!(sanitize_filename("invoice-2024.pdf"), "invoice-2024.pdf");
    }
}
