use crate::error::{ConnectedError, Result};
use quinn::{Connection, RecvStream, SendStream};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

const CHUNK_SIZE: usize = 64 * 1024; // 64KB chunks
const MAX_FILENAME_LEN: usize = 4096;

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
    /// File chunk
    Chunk { offset: u64, data: Vec<u8> },
    /// Transfer complete
    Complete { checksum: String },
    /// Acknowledge completion
    Ack,
    /// Error during transfer
    Error { message: String },
    /// Cancel transfer
    Cancel,
    /// Clipboard text sharing
    ClipboardText { text: String },
    /// Clipboard received acknowledgment
    ClipboardAck,
}

#[derive(Debug, Clone)]
pub enum TransferProgress {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferDirection {
    Sending,
    Receiving,
}

pub struct FileTransfer {
    connection: Connection,
}

impl FileTransfer {
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }

    /// Send a file to the connected peer
    pub async fn send_file<P: AsRef<Path>>(
        &self,
        file_path: P,
        progress_tx: Option<mpsc::UnboundedSender<TransferProgress>>,
    ) -> Result<()> {
        let path = file_path.as_ref();

        // Open and get file metadata
        let file = File::open(path).await?;
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
                debug!("Transfer accepted, starting to send chunks");
            }
            FileTransferMessage::Reject { reason } => {
                warn!("Transfer rejected: {}", reason);
                if let Some(ref tx) = progress_tx {
                    let _ = tx.send(TransferProgress::Failed {
                        error: format!("Rejected: {}", reason),
                    });
                }
                return Err(ConnectedError::PingFailed(format!(
                    "Transfer rejected: {}",
                    reason
                )));
            }
            _ => {
                return Err(ConnectedError::PingFailed(
                    "Unexpected response".to_string(),
                ));
            }
        }

        // Send file in chunks
        let mut reader = BufReader::new(file);
        let mut buffer = vec![0u8; CHUNK_SIZE];
        let mut offset: u64 = 0;
        let mut hasher = crc32fast::Hasher::new();

        loop {
            let bytes_read = reader.read(&mut buffer).await?;
            if bytes_read == 0 {
                break;
            }

            let chunk_data = buffer[..bytes_read].to_vec();
            hasher.update(&chunk_data);

            let chunk = FileTransferMessage::Chunk {
                offset,
                data: chunk_data,
            };

            send_message(&mut send, &chunk).await?;

            offset += bytes_read as u64;

            // Report progress
            if let Some(ref tx) = progress_tx {
                let _ = tx.send(TransferProgress::Progress {
                    bytes_transferred: offset,
                    total_size: file_size,
                });
            }

            debug!("Sent chunk: {}/{} bytes", offset, file_size);
        }

        // Send completion with checksum
        let checksum = format!("{:08x}", hasher.finalize());
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
                    let _ = tx.send(TransferProgress::Failed { error: message });
                }
                return Err(ConnectedError::PingFailed("Transfer failed".to_string()));
            }
            _ => {
                warn!("Unexpected response after completion");
            }
        }

        send.finish()?;
        Ok(())
    }

    /// Receive a file from the connected peer
    pub async fn receive_file<P: AsRef<Path>>(
        recv_stream: &mut RecvStream,
        send_stream: &mut SendStream,
        save_dir: P,
        progress_tx: Option<mpsc::UnboundedSender<TransferProgress>>,
        auto_accept: bool,
    ) -> Result<String> {
        // Receive the transfer request
        let request: FileTransferMessage = recv_message(recv_stream).await?;

        let (filename, file_size, _mime_type) = match request {
            FileTransferMessage::SendRequest {
                filename,
                size,
                mime_type,
            } => (filename, size, mime_type),
            _ => {
                return Err(ConnectedError::PingFailed(
                    "Expected SendRequest".to_string(),
                ));
            }
        };

        info!(
            "Incoming file transfer request: {} ({} bytes)",
            filename, file_size
        );

        // Notify progress
        if let Some(ref tx) = progress_tx {
            let _ = tx.send(TransferProgress::Starting {
                filename: filename.clone(),
                total_size: file_size,
            });
        }

        // Accept or reject (for now, auto-accept if flag is set)
        if !auto_accept {
            // In a real app, you'd prompt the user here
            let reject = FileTransferMessage::Reject {
                reason: "User declined".to_string(),
            };
            send_message(send_stream, &reject).await?;
            return Err(ConnectedError::PingFailed("User declined".to_string()));
        }

        let accept = FileTransferMessage::Accept;
        send_message(send_stream, &accept).await?;

        // Sanitize filename to prevent path traversal
        let safe_filename = sanitize_filename(&filename);
        let save_path = save_dir.as_ref().join(&safe_filename);

        // Create file
        let file = File::create(&save_path).await?;
        let mut writer = BufWriter::new(file);
        let mut bytes_received: u64 = 0;
        let mut hasher = crc32fast::Hasher::new();

        // Receive chunks
        loop {
            let message: FileTransferMessage = recv_message(recv_stream).await?;

            match message {
                FileTransferMessage::Chunk { offset, data } => {
                    if offset != bytes_received {
                        warn!(
                            "Chunk offset mismatch: expected {}, got {}",
                            bytes_received, offset
                        );
                    }

                    hasher.update(&data);
                    writer.write_all(&data).await?;
                    bytes_received += data.len() as u64;

                    if let Some(ref tx) = progress_tx {
                        let _ = tx.send(TransferProgress::Progress {
                            bytes_transferred: bytes_received,
                            total_size: file_size,
                        });
                    }

                    debug!("Received chunk: {}/{} bytes", bytes_received, file_size);
                }
                FileTransferMessage::Complete { checksum } => {
                    writer.flush().await?;

                    // Verify checksum
                    let our_checksum = format!("{:08x}", hasher.finalize());
                    if our_checksum != checksum {
                        error!(
                            "Checksum mismatch: expected {}, got {}",
                            checksum, our_checksum
                        );
                        let error = FileTransferMessage::Error {
                            message: "Checksum mismatch".to_string(),
                        };
                        send_message(send_stream, &error).await?;
                        return Err(ConnectedError::PingFailed("Checksum mismatch".to_string()));
                    }

                    // Send acknowledgment
                    let ack = FileTransferMessage::Ack;
                    send_message(send_stream, &ack).await?;

                    info!("File received successfully: {}", safe_filename);

                    if let Some(ref tx) = progress_tx {
                        let _ = tx.send(TransferProgress::Completed {
                            filename: safe_filename.clone(),
                            total_size: file_size,
                        });
                    }

                    return Ok(save_path.to_string_lossy().to_string());
                }
                FileTransferMessage::Cancel => {
                    warn!("Transfer cancelled by sender");
                    if let Some(ref tx) = progress_tx {
                        let _ = tx.send(TransferProgress::Cancelled);
                    }
                    // Clean up partial file
                    let _ = tokio::fs::remove_file(&save_path).await;
                    return Err(ConnectedError::PingFailed("Transfer cancelled".to_string()));
                }
                FileTransferMessage::Error { message } => {
                    error!("Transfer error from sender: {}", message);
                    if let Some(ref tx) = progress_tx {
                        let _ = tx.send(TransferProgress::Failed {
                            error: message.clone(),
                        });
                    }
                    // Clean up partial file
                    let _ = tokio::fs::remove_file(&save_path).await;
                    return Err(ConnectedError::PingFailed(message));
                }
                _ => {
                    warn!("Unexpected message during transfer");
                }
            }
        }
    }
}

/// Send a message over the stream
async fn send_message<T: Serialize>(stream: &mut SendStream, message: &T) -> Result<()> {
    let data = serde_json::to_vec(message)?;
    let len = data.len() as u32;

    // Send length prefix (4 bytes)
    stream.write_all(&len.to_be_bytes()).await?;
    // Send message data
    stream.write_all(&data).await?;

    Ok(())
}

/// Receive a message from the stream
async fn recv_message<T: for<'de> Deserialize<'de>>(stream: &mut RecvStream) -> Result<T> {
    // Read length prefix
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;

    if len > 10 * 1024 * 1024 {
        // 10MB max message size
        return Err(ConnectedError::PingFailed("Message too large".to_string()));
    }

    // Read message data
    let mut data = vec![0u8; len];
    stream.read_exact(&mut data).await?;

    let message = serde_json::from_slice(&data)?;
    Ok(message)
}

/// Sanitize filename to prevent path traversal attacks
fn sanitize_filename(filename: &str) -> String {
    let name = Path::new(filename)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unnamed");

    // Remove any remaining problematic characters
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("test.txt"), "test.txt");
        assert_eq!(sanitize_filename("../../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("file:name.txt"), "filename.txt");
        assert_eq!(
            sanitize_filename("normal-file_123.pdf"),
            "normal-file_123.pdf"
        );
    }
}
