use crate::error::{ConnectedError, Result};
// Use transport constants if public, or redefine
use bytes::BytesMut;
use quinn::{Connection, RecvStream, SendStream};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

// We need to match transport constants
const STREAM_TYPE_FILE: u8 = 2;
const BUFFER_SIZE: usize = 1024 * 1024; // 1MB Buffer for I/O

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
}

#[derive(Debug, Clone)]
pub enum TransferProgress {
    /// File transfer request received, waiting for user approval
    Pending {
        filename: String,
        total_size: u64,
        mime_type: Option<String>,
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

    /// Send a file to the connected peer using a new multiplexed stream
    pub async fn send_file<P: AsRef<Path>>(
        &self,
        file_path: P,
        progress_tx: Option<mpsc::UnboundedSender<TransferProgress>>,
    ) -> Result<()> {
        let mut path = file_path.as_ref().to_path_buf();
        let mut is_temp_file = false;

        // Check if directory
        if path.is_dir() {
            let dir_path = path.clone();
            let dir_name = dir_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("folder")
                .to_string();

            info!("Archiving directory: {}", dir_name);

            // Notify compression start (optional, maybe use a distinct state later)
            if let Some(ref tx) = progress_tx {
                let _ = tx.send(TransferProgress::Pending {
                    filename: format!("{}.zip", dir_name),
                    total_size: 0,
                    mime_type: Some("application/zip".to_string()),
                });
            }

            // Run compression in blocking thread
            let temp_archive_path =
                tokio::task::spawn_blocking(move || -> std::io::Result<std::path::PathBuf> {
                    let temp_dir = std::env::temp_dir();
                    let archive_name = format!("{}.zip", dir_name);
                    let archive_path = temp_dir.join(archive_name);

                    let file = std::fs::File::create(&archive_path)?;
                    let mut zip = zip::ZipWriter::new(file);
                    let options = zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated)
                        .unix_permissions(0o755);

                    let walk_dir = walkdir::WalkDir::new(&dir_path);
                    let it = walk_dir.into_iter();

                    for entry in it {
                        let entry = entry.map_err(std::io::Error::other)?;
                        let path = entry.path();
                        let name = path
                            .strip_prefix(dir_path.parent().unwrap_or(&dir_path))
                            .map_err(std::io::Error::other)?
                            .to_string_lossy()
                            .into_owned();

                        if path.is_file() {
                            zip.start_file(name, options)
                                .map_err(std::io::Error::other)?;
                            let mut f = std::fs::File::open(path)?;
                            std::io::copy(&mut f, &mut zip)?;
                        } else if !name.is_empty() {
                            zip.add_directory(name, options)
                                .map_err(std::io::Error::other)?;
                        }
                    }
                    zip.finish().map_err(std::io::Error::other)?;

                    Ok(archive_path)
                })
                .await
                .map_err(|e| ConnectedError::Io(std::io::Error::other(e)))??;

            path = temp_archive_path;
            is_temp_file = true;
        }

        // Open and get file metadata
        let mut file = match File::open(&path).await {
            Ok(f) => f,
            Err(e) => {
                if is_temp_file {
                    let _ = tokio::fs::remove_file(&path).await;
                }
                return Err(ConnectedError::Io(e));
            }
        };

        let metadata = match file.metadata().await {
            Ok(m) => m,
            Err(e) => {
                if is_temp_file {
                    let _ = tokio::fs::remove_file(&path).await;
                }
                return Err(ConnectedError::Io(e));
            }
        };
        let file_size = metadata.len();

        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| {
                if is_temp_file {
                    // Try to clean up best effort, though we can't await easily in closure if we panic but here we are ok
                }
                ConnectedError::InvalidAddress("Invalid filename".to_string())
            })?
            .to_string();

        info!("Starting file transfer: {} ({} bytes)", filename, file_size);

        // Notify progress
        if let Some(ref tx) = progress_tx {
            let _ = tx.send(TransferProgress::Starting {
                filename: filename.clone(),
                total_size: file_size,
            });
        }

        // Open bidirectional stream for file transfer

        let (mut send, mut recv) = match self.connection.open_bi().await {
            Ok(s) => s,

            Err(e) => {
                if is_temp_file {
                    let _ = tokio::fs::remove_file(&path).await;
                }

                return Err(ConnectedError::QuicConnection(e));
            }
        };

        // Write Stream Type Prefix

        if let Err(e) = send.write_all(&[STREAM_TYPE_FILE]).await {
            if is_temp_file {
                let _ = tokio::fs::remove_file(&path).await;
            }

            return Err(ConnectedError::QuicWrite(e));
        }

        // Send transfer request
        let request = FileTransferMessage::SendRequest {
            filename: filename.clone(),
            size: file_size,
            mime_type: mime_guess::from_path(&path).first().map(|m| m.to_string()),
        };

        if let Err(e) = send_message(&mut send, &request).await {
            if is_temp_file {
                let _ = tokio::fs::remove_file(&path).await;
            }
            return Err(e);
        }

        // Wait for accept/reject
        let response: FileTransferMessage = match recv_message(&mut recv).await {
            Ok(r) => r,
            Err(e) => {
                if is_temp_file {
                    let _ = tokio::fs::remove_file(&path).await;
                }
                return Err(e);
            }
        };

        match response {
            FileTransferMessage::Accept => {
                debug!("Transfer accepted, starting to stream data");
            }
            FileTransferMessage::Reject { reason } => {
                warn!("Transfer rejected: {}", reason);
                if let Some(ref tx) = progress_tx {
                    let _ = tx.send(TransferProgress::Failed {
                        error: format!("Rejected: {}", reason),
                    });
                }
                if is_temp_file {
                    let _ = tokio::fs::remove_file(&path).await;
                }
                return Err(ConnectedError::TransferRejected(reason));
            }
            _ => {
                if is_temp_file {
                    let _ = tokio::fs::remove_file(&path).await;
                }
                return Err(ConnectedError::Protocol(
                    "Unexpected response to file transfer request".to_string(),
                ));
            }
        }

        // Send file data (Raw Binary Stream)
        let mut offset: u64 = 0;
        let mut hasher = blake3::Hasher::new();
        let mut last_progress_update = std::time::Instant::now();
        let mut buf = BytesMut::with_capacity(BUFFER_SIZE);

        loop {
            let bytes_read = match file.read_buf(&mut buf).await {
                Ok(n) => n,
                Err(e) => {
                    if is_temp_file {
                        let _ = tokio::fs::remove_file(&path).await;
                    }
                    return Err(ConnectedError::Io(e));
                }
            };
            if bytes_read == 0 {
                break;
            }

            // `buf.split().freeze()` yields a `Bytes` chunk backed by the same allocation
            // (no copy) that QUIC can send without re-buffering in userland.
            let chunk = buf.split().freeze();

            hasher.update(&chunk);

            if let Err(e) = send.write_chunk(chunk).await {
                if is_temp_file {
                    let _ = tokio::fs::remove_file(&path).await;
                }

                return Err(ConnectedError::QuicWrite(e));
            }

            offset += bytes_read as u64;

            // Report progress (throttle to every 100ms)
            if last_progress_update.elapsed().as_millis() > 100 {
                if let Some(ref tx) = progress_tx {
                    let _ = tx.send(TransferProgress::Progress {
                        bytes_transferred: offset,
                        total_size: file_size,
                    });
                }
                last_progress_update = std::time::Instant::now();
            }
        }

        // Final progress update
        if let Some(ref tx) = progress_tx {
            let _ = tx.send(TransferProgress::Progress {
                bytes_transferred: offset,
                total_size: file_size,
            });
        }

        // Send completion with checksum
        let checksum = hasher.finalize().to_string();
        let complete = FileTransferMessage::Complete { checksum };
        if let Err(e) = send_message(&mut send, &complete).await {
            if is_temp_file {
                let _ = tokio::fs::remove_file(&path).await;
            }
            return Err(e);
        }

        // Wait for acknowledgment
        let ack: FileTransferMessage = match recv_message(&mut recv).await {
            Ok(r) => r,
            Err(e) => {
                if is_temp_file {
                    let _ = tokio::fs::remove_file(&path).await;
                }
                return Err(e);
            }
        };

        // Clean up temp file regardless of outcome now
        if is_temp_file {
            let _ = tokio::fs::remove_file(&path).await;
        }

        match ack {
            FileTransferMessage::Ack => {
                info!("File transfer completed successfully");
                if let Some(ref tx) = progress_tx {
                    let _ = tx.send(TransferProgress::Completed {
                        filename,
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

    /// Receive a file transfer on an existing stream
    /// This version reads the request, waits for accept/reject, then receives the file
    pub async fn handle_incoming(
        mut send: SendStream,
        mut recv: RecvStream,
        save_dir: impl AsRef<Path>,
        progress_tx: Option<mpsc::UnboundedSender<TransferProgress>>,
        auto_accept: bool,
        accept_rx: Option<tokio::sync::oneshot::Receiver<bool>>,
    ) -> Result<String> {
        // Read Request
        let request: FileTransferMessage = recv_message(&mut recv).await?;

        let (filename, file_size, _mime_type) = match request {
            FileTransferMessage::SendRequest {
                filename,
                size,
                mime_type,
            } => (filename, size, mime_type),
            _ => {
                return Err(ConnectedError::Protocol(
                    "Expected SendRequest message".to_string(),
                ));
            }
        };

        info!(
            "Incoming file transfer request: {} ({} bytes)",
            filename, file_size
        );

        // If we need user approval, emit Pending first
        if !auto_accept && let Some(ref tx) = progress_tx {
            let _ = tx.send(TransferProgress::Pending {
                filename: filename.clone(),
                total_size: file_size,
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
                Ok(Err(_)) => false, // Channel closed, treat as rejection
                Err(_) => false,     // Timeout, treat as rejection
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

        let accept = FileTransferMessage::Accept;
        send_message(&mut send, &accept).await?;

        // Now notify that transfer is starting
        if let Some(ref tx) = progress_tx {
            let _ = tx.send(TransferProgress::Starting {
                filename: filename.clone(),
                total_size: file_size,
            });
        }

        // Sanitize filename
        let safe_filename = sanitize_filename(&filename);
        let save_path = save_dir.as_ref().join(&safe_filename);

        // Create file
        let file = File::create(&save_path).await?;
        // Using `read_chunk` below yields `Bytes` without copying. Writing those directly to the
        // underlying file avoids an extra userland copy that `BufWriter` would introduce.
        let mut writer = file;
        let mut bytes_received: u64 = 0;
        let mut hasher = blake3::Hasher::new();
        let mut last_progress_update = std::time::Instant::now();

        // Read Raw Data
        // We read exactly file_size bytes
        let mut remaining = file_size;
        while remaining > 0 {
            let max_len = std::cmp::min(remaining, BUFFER_SIZE as u64) as usize;
            let Some(chunk) = recv.read_chunk(max_len, true).await? else {
                let _ = tokio::fs::remove_file(&save_path).await;
                return Err(ConnectedError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "unexpected EOF while receiving file data",
                )));
            };

            let bytes = chunk.bytes;
            let n = bytes.len() as u64;
            hasher.update(&bytes);
            writer.write_all(&bytes).await?;

            bytes_received += n;
            remaining -= n;

            if last_progress_update.elapsed().as_millis() > 100 {
                if let Some(ref tx) = progress_tx {
                    let _ = tx.send(TransferProgress::Progress {
                        bytes_transferred: bytes_received,
                        total_size: file_size,
                    });
                }
                last_progress_update = std::time::Instant::now();
            }
        }

        writer.flush().await?;

        // Read Completion Message
        let complete: FileTransferMessage = recv_message(&mut recv).await?;
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
                    let _ = tokio::fs::remove_file(&save_path).await;
                    return Err(ConnectedError::ChecksumMismatch);
                }

                let ack = FileTransferMessage::Ack;
                send_message(&mut send, &ack).await?;

                if let Some(ref tx) = progress_tx {
                    let _ = tx.send(TransferProgress::Completed {
                        filename: safe_filename.clone(),
                        total_size: file_size,
                    });
                }

                Ok(save_path.to_string_lossy().to_string())
            }
            FileTransferMessage::Cancel => {
                let _ = tokio::fs::remove_file(&save_path).await;
                if let Some(ref tx) = progress_tx {
                    let _ = tx.send(TransferProgress::Cancelled);
                }
                Err(ConnectedError::TransferFailed(
                    "Transfer cancelled by sender".to_string(),
                ))
            }
            FileTransferMessage::Error { message } => {
                let _ = tokio::fs::remove_file(&save_path).await;
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
}

/// Send a message over the stream
pub(crate) async fn send_message<T: Serialize>(stream: &mut SendStream, message: &T) -> Result<()> {
    let data = serde_json::to_vec(message)?;
    let len = data.len() as u32;
    stream.write_all(&len.to_be_bytes()).await?;
    stream.write_all(&data).await?;
    Ok(())
}

/// Receive a message from the stream
pub(crate) async fn recv_message<T: for<'de> Deserialize<'de>>(
    stream: &mut RecvStream,
) -> Result<T> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;

    if len > 10 * 1024 * 1024 {
        return Err(ConnectedError::PingFailed("Message too large".to_string()));
    }

    let mut data = vec![0u8; len];
    stream.read_exact(&mut data).await?;

    let message = serde_json::from_slice(&data)?;
    Ok(message)
}

fn sanitize_filename(filename: &str) -> String {
    let name = Path::new(filename)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unnamed");

    name.chars()
        .filter(|c| {
            !matches!(
                c,
                '/' | '\\' | '\0' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
            )
        })
        .take(255)
        .collect()
}
