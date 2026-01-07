use crate::error::{ConnectedError, Result};
// Use transport constants if public, or redefine
use quinn::{Connection, RecvStream, SendStream};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufWriter};
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
        let path = file_path.as_ref();

        // Open and get file metadata
        let mut file = File::open(path).await?;
        let metadata = file.metadata().await?;
        let file_size = metadata.len();

        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| ConnectedError::InvalidAddress("Invalid filename".to_string()))?
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
        let (mut send, mut recv) = self.connection.open_bi().await?;

        // Write Stream Type Prefix
        send.write_all(&[STREAM_TYPE_FILE]).await?;

        // Send transfer request
        let request = FileTransferMessage::SendRequest {
            filename: filename.clone(),
            size: file_size,
            mime_type: mime_guess::from_path(path).first().map(|m| m.to_string()),
        };

        send_message(&mut send, &request).await?;

        // Wait for accept/reject
        let response: FileTransferMessage = recv_message(&mut recv).await?;

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
                return Err(ConnectedError::TransferRejected(reason));
            }
            _ => {
                return Err(ConnectedError::Protocol(
                    "Unexpected response to file transfer request".to_string(),
                ));
            }
        }

        // Send file data (Raw Binary Stream)
        let mut buffer = vec![0u8; BUFFER_SIZE];
        let mut offset: u64 = 0;
        let mut hasher = blake3::Hasher::new();
        let mut last_progress_update = std::time::Instant::now();

        loop {
            let bytes_read = file.read(&mut buffer).await?;
            if bytes_read == 0 {
                break;
            }

            let chunk = &buffer[..bytes_read];
            hasher.update(chunk);
            send.write_all(chunk).await?;

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
        send_message(&mut send, &complete).await?;

        // Wait for acknowledgment
        let ack: FileTransferMessage = recv_message(&mut recv).await?;
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
        if !auto_accept {
            if let Some(ref tx) = progress_tx {
                let _ = tx.send(TransferProgress::Pending {
                    filename: filename.clone(),
                    total_size: file_size,
                    mime_type: _mime_type.clone(),
                });
            }
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
        let mut writer = BufWriter::new(file);
        let mut bytes_received: u64 = 0;
        let mut hasher = blake3::Hasher::new();

        let mut buffer = vec![0u8; BUFFER_SIZE];
        let mut last_progress_update = std::time::Instant::now();

        // Read Raw Data
        // We read exactly file_size bytes
        let mut remaining = file_size;
        while remaining > 0 {
            let to_read = std::cmp::min(remaining, BUFFER_SIZE as u64) as usize;
            // Read into buffer
            match recv.read_exact(&mut buffer[..to_read]).await {
                Ok(_) => {
                    let chunk = &buffer[..to_read];
                    hasher.update(chunk);
                    writer.write_all(chunk).await?;

                    bytes_received += to_read as u64;
                    remaining -= to_read as u64;

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
                Err(e) => {
                    let _ = tokio::fs::remove_file(&save_path).await;
                    return Err(ConnectedError::Io(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        e,
                    )));
                }
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
