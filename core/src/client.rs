use crate::device::{Device, DeviceType};
use crate::discovery::{DiscoveryEvent, DiscoveryService, DiscoverySource};
use crate::error::{ConnectedError, Result};
use crate::events::{ConnectedEvent, TransferDirection};
use crate::file_transfer::{FileTransfer, TransferProgress};
use crate::security::KeyStore;
use crate::transport::{Message, QuicTransport, UnpairReason};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;

use std::time::{Duration, Instant};

use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::{debug, error, info, warn};

type PendingTransferSender = oneshot::Sender<bool>;
type PendingHandshakeMap = HashMap<IpAddr, (Instant, Option<String>)>;

/// Capacity of the broadcast channel used for `ConnectedEvent`s.
///
/// A slow or stalled receiver (e.g. the UI poller) will miss events once the
/// channel is full — `broadcast` drops the oldest unsent items and returns
/// `RecvError::Lagged`.  512 gives a comfortable buffer for bursty discovery
/// and transfer-progress events while keeping memory usage modest.
const EVENT_CHANNEL_CAPACITY: usize = 512;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
const PAIRING_MODE_TIMEOUT: Duration = Duration::from_secs(120);

static RUSTLS_PROVIDER_INIT: OnceLock<bool> = OnceLock::new();

fn init_rustls_provider() {
    RUSTLS_PROVIDER_INIT.get_or_init(|| {
        // Both Ok (freshly installed) and Err (already installed by another call)
        // are success cases — the provider is available either way.
        let _ = rustls::crypto::ring::default_provider().install_default();
        true
    });
}

/// Maximum number of concurrent filesystem stream handlers.
/// Each handler can buffer up to 8 MB per request, so this caps the worst-case
/// memory consumption from FS streams at roughly MAX_CONCURRENT_FS_STREAMS × 8 MB.
const MAX_CONCURRENT_FS_STREAMS: usize = 16;

pub struct ConnectedClient {
    local_device: Device,
    discovery: Arc<DiscoveryService>,
    transport: Arc<QuicTransport>,
    event_tx: broadcast::Sender<ConnectedEvent>,
    key_store: Arc<RwLock<KeyStore>>,
    download_dir: Arc<RwLock<PathBuf>>,
    pairing_mode_handle: Arc<RwLock<Option<tokio::task::JoinHandle<()>>>>,
    pending_transfers: Arc<RwLock<HashMap<String, PendingTransferSender>>>,
    /// Track IPs we've sent handshakes to (with timestamp and expected peer fingerprint),
    /// so we can auto-trust their acks even if pairing mode times out.
    /// The Option<String> stores the peer's TLS fingerprint once known, to prevent
    /// auto-trusting a different device that happens to share or spoof the same IP.
    pending_handshakes: Arc<RwLock<PendingHandshakeMap>>,
    fs_provider: Arc<RwLock<Option<Box<dyn crate::filesystem::FilesystemProvider>>>>,
    /// Global semaphore bounding concurrent filesystem stream handlers to prevent
    /// memory exhaustion when many peers issue large read requests simultaneously.
    fs_stream_semaphore: Arc<tokio::sync::Semaphore>,
}

impl ConnectedClient {
    pub async fn new(
        device_name: String,
        device_type: DeviceType,
        port: u16,
        storage_path: Option<PathBuf>,
    ) -> Result<Arc<Self>> {
        let local_ip = get_local_ip().ok_or_else(|| {
            ConnectedError::InitializationError("Could not determine local IP".to_string())
        })?;

        Self::new_with_ip_and_bind(
            device_name,
            device_type,
            port,
            local_ip,
            local_ip,
            storage_path,
        )
        .await
    }

    pub async fn new_with_ip(
        device_name: String,
        device_type: DeviceType,
        port: u16,
        local_ip: IpAddr,
        storage_path: Option<PathBuf>,
    ) -> Result<Arc<Self>> {
        Self::new_with_ip_and_bind(
            device_name,
            device_type,
            port,
            local_ip,
            local_ip,
            storage_path,
        )
        .await
    }

    pub async fn new_with_bind_all(
        device_name: String,
        device_type: DeviceType,
        port: u16,
        storage_path: Option<PathBuf>,
    ) -> Result<Arc<Self>> {
        let local_ip = get_local_ip().unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        let bind_ip = IpAddr::V4(Ipv4Addr::UNSPECIFIED);

        Self::new_with_ip_and_bind(
            device_name,
            device_type,
            port,
            local_ip,
            bind_ip,
            storage_path,
        )
        .await
    }

    pub async fn new_with_bind_ip(
        device_name: String,
        device_type: DeviceType,
        port: u16,
        local_ip: IpAddr,
        bind_ip: IpAddr,
        storage_path: Option<PathBuf>,
    ) -> Result<Arc<Self>> {
        Self::new_with_ip_and_bind(
            device_name,
            device_type,
            port,
            local_ip,
            bind_ip,
            storage_path,
        )
        .await
    }

    async fn new_with_ip_and_bind(
        device_name: String,
        device_type: DeviceType,
        port: u16,
        local_ip: IpAddr,
        bind_ip: IpAddr,
        storage_path: Option<PathBuf>,
    ) -> Result<Arc<Self>> {
        init_rustls_provider();

        // Load KeyStore first to get the persisted device_id
        let key_store = Arc::new(RwLock::new(KeyStore::new(storage_path.clone())?));
        let device_id = key_store.read().device_id().to_string();

        let download_dir = if let Some(p) = storage_path {
            p.join("downloads")
        } else {
            dirs::download_dir().unwrap_or_else(std::env::temp_dir)
        };

        if !download_dir.exists() {
            std::fs::create_dir_all(&download_dir).map_err(ConnectedError::Io)?;
        }

        let download_dir = Arc::new(RwLock::new(download_dir));

        // 1. Initialize Transport (QUIC)
        // If port is 0, OS assigns one. We need to know it for mDNS.
        let bind_addr = SocketAddr::new(bind_ip, port);
        let transport = QuicTransport::new(bind_addr, device_id.clone(), key_store.clone()).await?;
        let actual_port = transport.local_addr()?.port();

        // 2. Initialize Device Info
        let local_device = Device::new(device_id, device_name, local_ip, actual_port, device_type);

        // 3. Initialize Discovery (mDNS)
        let discovery = DiscoveryService::new(local_device.clone())
            .map_err(|e| ConnectedError::InitializationError(e.to_string()))?;

        // 4. Create Event Bus
        let (event_tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);

        let client = Arc::new(Self {
            local_device,
            discovery: Arc::new(discovery),
            transport: Arc::new(transport),
            event_tx,
            key_store,
            download_dir,
            pairing_mode_handle: Arc::new(RwLock::new(None)),
            pending_transfers: Arc::new(RwLock::new(HashMap::new())),
            pending_handshakes: Arc::new(RwLock::new(HashMap::new())),
            fs_provider: Arc::new(RwLock::new(None)),
            fs_stream_semaphore: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_FS_STREAMS)),
        });

        // 5. Start Background Tasks
        client.start_background_tasks().await?;

        // 6. Announce Presence
        client.discovery.announce()?;

        Ok(client)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ConnectedEvent> {
        self.event_tx.subscribe()
    }

    pub fn local_device(&self) -> &Device {
        &self.local_device
    }

    pub fn get_discovered_devices(&self) -> Vec<Device> {
        self.discovery.get_discovered_devices()
    }

    pub fn clear_discovered_devices(&self) {
        self.discovery.clear_discovered_devices()
    }

    /// Lightweight discovery refresh: clears the stale device list and
    /// re-announces ourselves on mDNS so that nearby peers re-discover us
    /// (and the running browse loop re-discovers them).
    /// Update the directory where incoming file transfers are saved.
    pub fn set_download_dir(&self, path: PathBuf) -> Result<()> {
        if !path.exists() {
            std::fs::create_dir_all(&path).map_err(ConnectedError::Io)?;
        }
        info!("Download directory changed to: {}", path.display());
        *self.download_dir.write() = path;
        Ok(())
    }

    /// Get the current download directory.
    pub fn get_download_dir(&self) -> PathBuf {
        self.download_dir.read().clone()
    }

    pub fn refresh_discovery(&self) {
        info!("Refreshing discovery — clearing stale devices and re-announcing");
        self.discovery.clear_discovered_devices();
        if let Err(e) = self.discovery.announce() {
            warn!("Failed to re-announce during discovery refresh: {}", e);
        }
    }

    pub fn inject_proximity_device(
        &self,
        device_id: String,
        device_name: String,
        device_type: DeviceType,
        ip: IpAddr,
        port: u16,
    ) -> Result<()> {
        if device_id == self.local_device.id {
            return Ok(());
        }

        let device = Device::new(device_id, device_name, ip, port, device_type);
        if let Some(event) = self
            .discovery
            .upsert_device_endpoint(device, DiscoverySource::Discovered)
        {
            match event {
                DiscoveryEvent::DeviceFound(d) => {
                    let _ = self.event_tx.send(ConnectedEvent::DeviceFound(d));
                }
                DiscoveryEvent::DeviceLost(id) => {
                    let _ = self.event_tx.send(ConnectedEvent::DeviceLost(id));
                }
                DiscoveryEvent::Error(msg) => {
                    let _ = self
                        .event_tx
                        .send(ConnectedEvent::Error(format!("Discovery error: {}", msg)));
                }
            }
        }

        Ok(())
    }

    pub fn get_fingerprint(&self) -> String {
        self.key_store.read().fingerprint()
    }

    pub fn set_pairing_mode(&self, enabled: bool) {
        // Cancel any existing pairing mode timeout
        if let Some(handle) = self.pairing_mode_handle.write().take() {
            handle.abort();
        }

        self.key_store.write().set_pairing_mode(enabled);
        let _ = self
            .event_tx
            .send(ConnectedEvent::PairingModeChanged(enabled));

        // If enabling, set up auto-disable timer
        if enabled {
            let key_store = self.key_store.clone();
            let event_tx = self.event_tx.clone();
            let handle = tokio::spawn(async move {
                tokio::time::sleep(PAIRING_MODE_TIMEOUT).await;
                key_store.write().set_pairing_mode(false);
                let _ = event_tx.send(ConnectedEvent::PairingModeChanged(false));
                info!("Pairing mode automatically disabled after timeout");
            });
            *self.pairing_mode_handle.write() = Some(handle);
        }
    }

    pub fn register_filesystem_provider(
        &self,
        provider: Box<dyn crate::filesystem::FilesystemProvider>,
    ) {
        *self.fs_provider.write() = Some(provider);
    }

    pub async fn fs_list_dir(
        &self,
        target_ip: IpAddr,
        target_port: u16,
        path: String,
    ) -> Result<Vec<crate::filesystem::FsEntry>> {
        use crate::filesystem::{FilesystemMessage, STREAM_TYPE_FS};

        let addr = SocketAddr::new(target_ip, target_port);
        let (mut send, mut recv) = self.transport.open_stream(addr, STREAM_TYPE_FS).await?;

        let req = FilesystemMessage::ListDirRequest { path };
        crate::file_transfer::send_message(&mut send, &req).await?;

        let resp: FilesystemMessage =
            crate::file_transfer::recv_message_with_limit(&mut recv, 8 * 1024 * 1024).await?;

        match resp {
            FilesystemMessage::ListDirResponse { entries } => Ok(entries),
            FilesystemMessage::Error { message } => Err(ConnectedError::Protocol(message)),
            _ => Err(ConnectedError::Protocol("Unexpected response".to_string())),
        }
    }

    pub async fn fs_download_file(
        &self,
        target_ip: IpAddr,
        target_port: u16,
        remote_path: String,
        local_path: PathBuf,
    ) -> Result<u64> {
        use crate::filesystem::{FilesystemMessage, STREAM_TYPE_FS};
        use tokio::io::AsyncWriteExt;

        let addr = SocketAddr::new(target_ip, target_port);
        let (mut send, mut recv) = self.transport.open_stream(addr, STREAM_TYPE_FS).await?;

        // 1. Get Metadata to know size (optional, but good for allocation or progress)
        let meta_req = FilesystemMessage::GetMetadataRequest {
            path: remote_path.clone(),
        };
        crate::file_transfer::send_message(&mut send, &meta_req).await?;
        let meta_resp: FilesystemMessage =
            crate::file_transfer::recv_message_with_limit(&mut recv, 8 * 1024 * 1024).await?;

        let file_size = match meta_resp {
            FilesystemMessage::GetMetadataResponse { entry } => entry.size,
            FilesystemMessage::Error { message } => return Err(ConnectedError::Protocol(message)),
            _ => {
                return Err(ConnectedError::Protocol(
                    "Unexpected metadata response".to_string(),
                ));
            }
        };

        // Create local file
        let mut file = tokio::fs::File::create(&local_path)
            .await
            .map_err(ConnectedError::Io)?;

        let mut offset = 0u64;
        let chunk_size = 1024 * 1024; // 1MB chunks

        while offset < file_size {
            let size = std::cmp::min(chunk_size, file_size - offset);
            let req = FilesystemMessage::ReadFileRequest {
                path: remote_path.clone(),
                offset,
                size,
            };
            crate::file_transfer::send_message(&mut send, &req).await?;

            let resp: FilesystemMessage =
                crate::file_transfer::recv_message_with_limit(&mut recv, 8 * 1024 * 1024).await?;

            match resp {
                FilesystemMessage::ReadFileResponse { data } => {
                    if data.is_empty() {
                        let _ = tokio::fs::remove_file(&local_path).await;
                        return Err(ConnectedError::Io(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "unexpected EOF while receiving file data",
                        )));
                    }
                    file.write_all(&data).await.map_err(ConnectedError::Io)?;
                    offset += data.len() as u64;
                }
                FilesystemMessage::Error { message } => {
                    let _ = tokio::fs::remove_file(&local_path).await;
                    return Err(ConnectedError::Protocol(message));
                }
                _ => {
                    let _ = tokio::fs::remove_file(&local_path).await;
                    return Err(ConnectedError::Protocol("Unexpected response".to_string()));
                }
            }
        }

        if offset != file_size {
            let _ = tokio::fs::remove_file(&local_path).await;
            return Err(ConnectedError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "file download incomplete",
            )));
        }

        file.flush().await.map_err(ConnectedError::Io)?;
        Ok(offset)
    }

    /// Download a file with progress callback
    pub async fn fs_download_file_with_progress<F>(
        &self,
        target_ip: IpAddr,
        target_port: u16,
        remote_path: String,
        local_path: PathBuf,
        progress_callback: F,
    ) -> Result<u64>
    where
        F: Fn(u64, u64) + Send + Sync,
    {
        use crate::filesystem::{FilesystemMessage, STREAM_TYPE_FS};
        use tokio::io::AsyncWriteExt;

        let addr = SocketAddr::new(target_ip, target_port);
        let (mut send, mut recv) = self.transport.open_stream(addr, STREAM_TYPE_FS).await?;

        // Get metadata to know total size
        let meta_req = FilesystemMessage::GetMetadataRequest {
            path: remote_path.clone(),
        };
        crate::file_transfer::send_message(&mut send, &meta_req).await?;
        let meta_resp: FilesystemMessage =
            crate::file_transfer::recv_message_with_limit(&mut recv, 8 * 1024 * 1024).await?;

        let file_size = match meta_resp {
            FilesystemMessage::GetMetadataResponse { entry } => entry.size,
            FilesystemMessage::Error { message } => return Err(ConnectedError::Protocol(message)),
            _ => {
                return Err(ConnectedError::Protocol(
                    "Unexpected metadata response".to_string(),
                ));
            }
        };

        // Report initial progress
        progress_callback(0, file_size);

        // Create local file
        let mut file = tokio::fs::File::create(&local_path)
            .await
            .map_err(ConnectedError::Io)?;

        let mut offset = 0u64;
        let chunk_size = 2 * 1024 * 1024; // 2MB chunks (safe for JSON+base64 encoding overhead)

        while offset < file_size {
            let size = std::cmp::min(chunk_size, file_size - offset);
            let req = FilesystemMessage::ReadFileRequest {
                path: remote_path.clone(),
                offset,
                size,
            };
            crate::file_transfer::send_message(&mut send, &req).await?;

            let resp: FilesystemMessage =
                crate::file_transfer::recv_message_with_limit(&mut recv, 8 * 1024 * 1024).await?;

            match resp {
                FilesystemMessage::ReadFileResponse { data } => {
                    if data.is_empty() {
                        let _ = tokio::fs::remove_file(&local_path).await;
                        return Err(ConnectedError::Io(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "unexpected EOF while receiving file data",
                        )));
                    }
                    file.write_all(&data).await.map_err(ConnectedError::Io)?;
                    offset += data.len() as u64;
                    progress_callback(offset, file_size);
                }
                FilesystemMessage::Error { message } => {
                    let _ = tokio::fs::remove_file(&local_path).await;
                    return Err(ConnectedError::Protocol(message));
                }
                _ => {
                    let _ = tokio::fs::remove_file(&local_path).await;
                    return Err(ConnectedError::Protocol("Unexpected response".to_string()));
                }
            }
        }

        if offset != file_size {
            let _ = tokio::fs::remove_file(&local_path).await;
            return Err(ConnectedError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "file download incomplete",
            )));
        }

        file.flush().await.map_err(ConnectedError::Io)?;
        Ok(offset)
    }

    /// Download a folder by recursively downloading all files with progress
    pub async fn fs_download_folder_with_progress<F>(
        &self,
        target_ip: IpAddr,
        target_port: u16,
        remote_path: String,
        local_path: PathBuf,
        progress_callback: F,
    ) -> Result<u64>
    where
        F: Fn(u64, u64, &str) + Send + Sync,
    {
        use std::sync::atomic::{AtomicU64, Ordering};

        // First, scan the folder to get total size and file list
        let (files, total_size) = self
            .scan_remote_folder(target_ip, target_port, &remote_path)
            .await?;

        if files.is_empty() {
            return Ok(0);
        }

        // Get the folder name from remote path to preserve it
        let folder_name = std::path::Path::new(&remote_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("folder");

        // Create local directory WITH the folder name
        let local_folder_path = local_path.join(folder_name);
        tokio::fs::create_dir_all(&local_folder_path)
            .await
            .map_err(ConnectedError::Io)?;

        // Get parent path of remote folder for calculating relative paths
        let remote_parent = std::path::Path::new(&remote_path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        // Helper: sanitize a relative path from the remote peer to prevent path traversal.
        // Strips leading '/', removes '..' components, and verifies the result stays
        // inside `base_dir`.
        fn sanitize_relative_path(
            raw: &str,
            remote_parent: &str,
            base_dir: &std::path::Path,
        ) -> std::result::Result<String, ConnectedError> {
            use std::path::{Component, Path};

            let stripped = if remote_parent.is_empty() {
                raw.trim_start_matches('/').to_string()
            } else {
                raw.strip_prefix(remote_parent)
                    .unwrap_or(raw)
                    .trim_start_matches('/')
                    .to_string()
            };

            // Normalize: keep only Normal components (reject ParentDir / RootDir / Prefix)
            let safe: std::path::PathBuf = Path::new(&stripped)
                .components()
                .filter_map(|c| match c {
                    Component::Normal(seg) => Some(seg),
                    _ => None, // drop '..', '/', prefix, '.'
                })
                .collect();

            if safe.as_os_str().is_empty() {
                return Err(ConnectedError::Filesystem(
                    "Remote path is empty after sanitization".to_string(),
                ));
            }

            // Final safety check: the joined path must still be inside base_dir
            let full = base_dir.join(&safe);
            // Use lexical check (canonicalize may fail for not-yet-created paths)
            if !full.starts_with(base_dir) {
                return Err(ConnectedError::Filesystem(format!(
                    "Path escapes target directory: {}",
                    safe.display()
                )));
            }

            Ok(safe.to_string_lossy().to_string())
        }

        // Pre-create all directories
        for (file_remote_path, _) in &files {
            let relative_path =
                sanitize_relative_path(file_remote_path, &remote_parent, &local_path)?;
            let local_file_path = local_path.join(&relative_path);
            if let Some(parent) = local_file_path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(ConnectedError::Io)?;
            }
        }

        let addr = SocketAddr::new(target_ip, target_port);
        let bytes_downloaded = Arc::new(AtomicU64::new(0));
        let transport = self.transport.clone();

        // Download files with concurrency limit - higher = faster on good connections
        const MAX_CONCURRENT_DOWNLOADS: usize = 8;
        let semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_DOWNLOADS));

        let download_tasks: Vec<_> = files
            .into_iter()
            .map(|(file_remote_path, file_size)| {
                let semaphore = semaphore.clone();
                let bytes_downloaded = bytes_downloaded.clone();
                let remote_parent = remote_parent.clone();
                let local_path = local_path.clone();
                let transport = transport.clone();

                async move {
                    let _permit = semaphore
                        .acquire()
                        .await
                        .map_err(|_| ConnectedError::Protocol("Semaphore closed".to_string()))?;

                    // Calculate relative path INCLUDING the folder name
                    // Sanitize to prevent path traversal from malicious remote paths
                    let relative_path =
                        sanitize_relative_path(&file_remote_path, &remote_parent, &local_path)?;
                    let local_file_path = local_path.join(&relative_path);

                    let file_name = local_file_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown")
                        .to_string();

                    // Skip empty files
                    if file_size == 0 {
                        tokio::fs::File::create(&local_file_path)
                            .await
                            .map_err(ConnectedError::Io)?;
                        return Ok::<(String, u64), ConnectedError>((file_name, 0));
                    }

                    // Download the file
                    let downloaded = Self::download_single_file(
                        &transport,
                        addr,
                        &file_remote_path,
                        file_size,
                        &local_file_path,
                        &bytes_downloaded,
                    )
                    .await?;

                    Ok((file_name, downloaded))
                }
            })
            .collect();

        // Run downloads and report progress
        let progress_callback = &progress_callback;
        let mut last_progress_report = std::time::Instant::now();

        let join_handles: Vec<_> = download_tasks
            .into_iter()
            .map(|task| tokio::spawn(task))
            .collect();

        // Poll progress while downloads are running
        loop {
            // Report progress less frequently to reduce overhead
            let current_bytes = bytes_downloaded.load(Ordering::Relaxed);
            if last_progress_report.elapsed() > Duration::from_millis(250) {
                progress_callback(current_bytes, total_size, "downloading...");
                last_progress_report = std::time::Instant::now();
            }

            // Check if all tasks are done
            let mut all_done = true;
            for handle in &join_handles {
                if !handle.is_finished() {
                    all_done = false;
                    break;
                }
            }

            if all_done {
                break;
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Collect results and check for errors
        for handle in join_handles {
            match handle.await {
                Ok(Ok((file_name, _))) => {
                    let current = bytes_downloaded.load(Ordering::Relaxed);
                    progress_callback(current, total_size, &file_name);
                }
                Ok(Err(e)) => return Err(e),
                Err(e) => {
                    return Err(ConnectedError::Protocol(format!(
                        "Download task panicked: {}",
                        e
                    )));
                }
            }
        }

        let final_bytes = bytes_downloaded.load(Ordering::Relaxed);
        progress_callback(final_bytes, total_size, "complete");
        Ok(final_bytes)
    }

    /// Download a single file - helper for parallel downloads
    async fn download_single_file(
        transport: &QuicTransport,
        addr: SocketAddr,
        remote_path: &str,
        file_size: u64,
        local_path: &std::path::Path,
        bytes_counter: &Arc<std::sync::atomic::AtomicU64>,
    ) -> Result<u64> {
        use crate::filesystem::{FilesystemMessage, STREAM_TYPE_FS};
        use std::sync::atomic::Ordering;
        use tokio::io::AsyncWriteExt;

        let (mut send, mut recv) = transport.open_stream(addr, STREAM_TYPE_FS).await?;

        let mut file = tokio::fs::File::create(local_path)
            .await
            .map_err(ConnectedError::Io)?;

        let mut offset = 0u64;
        let chunk_size = 2 * 1024 * 1024; // 2MB chunks (safe for JSON+base64 encoding overhead)

        while offset < file_size {
            let size = std::cmp::min(chunk_size, file_size - offset);
            let req = FilesystemMessage::ReadFileRequest {
                path: remote_path.to_string(),
                offset,
                size,
            };
            crate::file_transfer::send_message(&mut send, &req).await?;

            let resp: FilesystemMessage =
                crate::file_transfer::recv_message_with_limit(&mut recv, 8 * 1024 * 1024).await?;

            match resp {
                FilesystemMessage::ReadFileResponse { data } => {
                    if data.is_empty() && offset < file_size {
                        let _ = tokio::fs::remove_file(local_path).await;
                        return Err(ConnectedError::Io(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "unexpected EOF while receiving file data",
                        )));
                    }
                    let data_len = data.len() as u64;
                    file.write_all(&data).await.map_err(ConnectedError::Io)?;
                    offset += data_len;
                    bytes_counter.fetch_add(data_len, Ordering::Relaxed);
                }
                FilesystemMessage::Error { message } => {
                    let _ = tokio::fs::remove_file(local_path).await;
                    return Err(ConnectedError::Protocol(message));
                }
                _ => {
                    let _ = tokio::fs::remove_file(local_path).await;
                    return Err(ConnectedError::Protocol("Unexpected response".to_string()));
                }
            }
        }

        file.flush().await.map_err(ConnectedError::Io)?;
        Ok(offset)
    }

    /// Recursively scan a remote folder to get all files and total size
    async fn scan_remote_folder(
        &self,
        target_ip: IpAddr,
        target_port: u16,
        path: &str,
    ) -> Result<(Vec<(String, u64)>, u64)> {
        use crate::filesystem::FsEntryType;

        // Limits to prevent enumeration DoS from a malicious peer that returns
        // huge directory listings or deeply nested structures.
        const MAX_FILES: usize = 50_000;
        const MAX_DEPTH: usize = 64;

        let mut files = Vec::new();
        let mut total_size = 0u64;
        // Each entry is (path, current_depth)
        let mut dirs_to_scan: Vec<(String, usize)> = vec![(path.to_string(), 0)];

        while let Some((dir_path, depth)) = dirs_to_scan.pop() {
            if depth >= MAX_DEPTH {
                warn!(
                    "scan_remote_folder: skipping '{}' — max depth {} reached",
                    dir_path, MAX_DEPTH
                );
                continue;
            }

            let entries = self
                .fs_list_dir(target_ip, target_port, dir_path.clone())
                .await?;

            for entry in entries {
                match entry.entry_type {
                    FsEntryType::File => {
                        files.push((entry.path, entry.size));
                        total_size += entry.size;

                        if files.len() >= MAX_FILES {
                            warn!(
                                "scan_remote_folder: file limit {} reached, stopping scan",
                                MAX_FILES
                            );
                            return Ok((files, total_size));
                        }
                    }
                    FsEntryType::Directory => {
                        dirs_to_scan.push((entry.path, depth + 1));
                    }
                    _ => {}
                }
            }
        }

        Ok((files, total_size))
    }

    pub async fn fs_get_thumbnail(
        &self,
        target_ip: IpAddr,
        target_port: u16,
        path: String,
    ) -> Result<Vec<u8>> {
        use crate::filesystem::{FilesystemMessage, STREAM_TYPE_FS};

        let addr = SocketAddr::new(target_ip, target_port);
        let (mut send, mut recv) = self.transport.open_stream(addr, STREAM_TYPE_FS).await?;

        let req = FilesystemMessage::GetThumbnailRequest { path };
        crate::file_transfer::send_message(&mut send, &req).await?;

        let resp: FilesystemMessage =
            crate::file_transfer::recv_message_with_limit(&mut recv, 8 * 1024 * 1024).await?;

        match resp {
            FilesystemMessage::GetThumbnailResponse { data } => Ok(data),
            FilesystemMessage::Error { message } => Err(ConnectedError::Protocol(message)),
            _ => Err(ConnectedError::Protocol("Unexpected response".to_string())),
        }
    }

    pub fn is_pairing_mode(&self) -> bool {
        self.key_store.read().is_pairing_mode()
    }

    pub fn accept_file_transfer(&self, transfer_id: &str) -> Result<()> {
        let sender = self.pending_transfers.write().remove(transfer_id);
        if let Some(tx) = sender {
            let _ = tx.send(true);
            Ok(())
        } else {
            Err(ConnectedError::Protocol(format!(
                "No pending transfer with id: {}",
                transfer_id
            )))
        }
    }

    pub fn reject_file_transfer(&self, transfer_id: &str) -> Result<()> {
        let sender = self.pending_transfers.write().remove(transfer_id);
        if let Some(tx) = sender {
            let _ = tx.send(false);
            Ok(())
        } else {
            Err(ConnectedError::Protocol(format!(
                "No pending transfer with id: {}",
                transfer_id
            )))
        }
    }

    pub async fn reject_pairing(&self, device_id: &str) -> Result<()> {
        let target = self
            .discovery
            .get_device_by_id(device_id)
            .and_then(|d| d.ip_addr().map(|ip| (ip, d.port)));

        if let Some((ip, port)) = target {
            // Send explicit rejection message
            let _ = self.send_handshake_reject(ip, port).await;

            // Wait briefly to ensure the rejection message is transmitted before closing
            tokio::time::sleep(Duration::from_millis(300)).await;

            // Then invalidate connection
            let addr = SocketAddr::new(ip, port);
            self.transport
                .invalidate_connection_with_reason(&addr, b"rejected");
        }

        self.invalidate_connection_by_device_id_with_reason(device_id, b"rejected");

        info!("Rejected pairing request from {}", device_id);
        Ok(())
    }

    pub async fn send_handshake_reject(&self, target_ip: IpAddr, target_port: u16) -> Result<()> {
        let addr = SocketAddr::new(target_ip, target_port);

        let (mut send, _recv) = match self
            .transport
            .open_stream(addr, QuicTransport::STREAM_TYPE_CONTROL)
            .await
        {
            Ok(s) => s,
            Err(e) => {
                warn!("Could not send handshake reject: {}", e);
                return Ok(());
            }
        };

        let msg = Message::HandshakeReject {
            device_id: self.local_device.id.clone(),
            reason: None,
        };

        let data = serde_json::to_vec(&msg)
            .map_err(|e| ConnectedError::InitializationError(e.to_string()))?;
        let len_bytes = (data.len() as u32).to_be_bytes();

        let _ = send.write_all(&len_bytes).await;
        let _ = send.write_all(&data).await;
        let _ = send.finish();

        info!("Sent handshake reject to {}:{}", target_ip, target_port);
        Ok(())
    }

    pub fn trust_device(
        &self,
        fingerprint: String,
        device_id: Option<String>,
        name: String,
    ) -> Result<()> {
        // We don't have the device ID here usually unless we cached the handshake request.
        // For now, we trust by fingerprint. The ID can be updated later or passed if known.
        self.key_store
            .write()
            .trust_peer(fingerprint, device_id, Some(name))
    }

    pub fn is_device_trusted(&self, device_id: &str) -> bool {
        self.key_store
            .read()
            .get_trusted_peers()
            .iter()
            .any(|p| p.device_id.as_deref() == Some(device_id))
    }

    pub fn get_trusted_peers(&self) -> Vec<crate::security::PeerInfo> {
        self.key_store.read().get_trusted_peers()
    }

    pub async fn send_ping(&self, target_ip: IpAddr, target_port: u16) -> Result<u64> {
        let addr = SocketAddr::new(target_ip, target_port);
        let rtt = self.transport.send_ping(addr).await?;
        Ok(rtt.as_millis() as u64)
    }

    pub async fn send_handshake(&self, target_ip: IpAddr, target_port: u16) -> Result<()> {
        let addr = SocketAddr::new(target_ip, target_port);

        // Ensure pairing mode is enabled for outgoing handshakes
        if !self.is_pairing_mode() {
            self.set_pairing_mode(true);
        }

        // Track this as a pending handshake so we can auto-trust acks even if pairing mode times out.
        // The fingerprint is initially None and will be filled in by send_handshake_internal
        // once the TLS connection is established and the peer's cert is available.
        self.pending_handshakes
            .write()
            .insert(target_ip, (Instant::now(), None));

        // Try to send the handshake.
        // If the connection is stale, send_handshake_internal (specifically open_stream) will likely fail.
        // We catch that failure and retry once with a fresh connection.
        // We DO NOT wrap the entire call in a timeout because waiting for user approval takes time.

        let result = self.send_handshake_internal(addr).await;

        let result = match result {
            Ok(()) => Ok(()),
            Err(e) => {
                // Check if this is a connection/IO error that warrants a retry with a fresh connection
                let should_retry =
                    matches!(e, ConnectedError::Connection(_) | ConnectedError::Io(_));

                if should_retry {
                    info!(
                        "Handshake failed to {}: {}, invalidating and retrying...",
                        addr, e
                    );

                    self.transport.invalidate_connection(&addr);
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    self.send_handshake_internal(addr).await
                } else {
                    Err(e)
                }
            }
        };

        // Cleanup pending state
        match &result {
            Ok(()) | Err(ConnectedError::PairingFailed(_)) => {
                self.pending_handshakes.write().remove(&target_ip);
            }
            Err(ConnectedError::Timeout(_)) => {
                // Keep in pending_handshakes - background handler may receive ack later
                let pending = self.pending_handshakes.clone();
                let ip = target_ip;
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(300)).await; // 5 min cleanup
                    pending.write().remove(&ip);
                });
            }
            _ => {
                self.pending_handshakes.write().remove(&target_ip);
            }
        }

        result
    }

    async fn send_handshake_internal(&self, addr: SocketAddr) -> Result<()> {
        let (mut send, mut recv) = self
            .transport
            .open_stream(addr, QuicTransport::STREAM_TYPE_CONTROL)
            .await?;

        // Get fingerprint immediately after connecting, before the connection might close
        let peer_fingerprint = self.transport.get_connection_fingerprint(addr).await;
        if peer_fingerprint.is_none() {
            warn!(
                "Could not get peer fingerprint immediately after connecting to {}",
                addr
            );
        }

        // Store the peer fingerprint in the pending handshake entry so the
        // background handler can verify it when auto-trusting acks/handshakes.
        if let Some(ref fp) = peer_fingerprint
            && let Some(entry) = self.pending_handshakes.write().get_mut(&addr.ip())
        {
            entry.1 = Some(fp.clone());
        }

        let msg = Message::Handshake {
            device_id: self.local_device.id.clone(),
            device_name: self.local_device.name.clone(),
            listening_port: self.local_device.port,
        };

        let data = serde_json::to_vec(&msg)
            .map_err(|e| ConnectedError::InitializationError(e.to_string()))?;
        let len_bytes = (data.len() as u32).to_be_bytes();

        send.write_all(&len_bytes)
            .await
            .map_err(|e| ConnectedError::Io(e.into()))?;
        send.write_all(&data)
            .await
            .map_err(|e| ConnectedError::Io(e.into()))?;

        // Wait for HandshakeAck with timeout, but also check if trust was established
        // via the background handler (trust confirmation on a different stream)
        let key_store = self.key_store.clone();
        let transport = self.transport.clone();
        let target_addr = addr;

        let ack_result = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            // First, try to read from the stream (the receiver might respond directly)
            let read_result: std::result::Result<Message, ConnectedError> = async {
                let mut len_buf = [0u8; 4];
                recv.read_exact(&mut len_buf).await?;
                let msg_len = u32::from_be_bytes(len_buf) as usize;
                if msg_len > 64 * 1024 {
                    return Err(ConnectedError::Protocol("Message too large".to_string()));
                }
                let mut data = vec![0u8; msg_len];
                recv.read_exact(&mut data).await?;
                let response: Message = serde_json::from_slice(&data)?;
                Ok(response)
            }
            .await;

            match read_result {
                Ok(msg) => return Ok(msg),
                Err(e) => {
                    // Distinguish connection-level failures (TLS rejection, connection
                    // closed) from stream-level events (receiver closed the stream to
                    // show a pairing dialog).  If the QUIC connection itself is gone,
                    // there is no point entering the polling loop — the remote device
                    // will never send a trust confirmation on this connection.
                    let connection_lost = match &e {
                        ConnectedError::QuicRead(read_exact_err) => {
                            matches!(
                                read_exact_err,
                                quinn::ReadExactError::ReadError(quinn::ReadError::ConnectionLost(
                                    _
                                ))
                            )
                        }
                        ConnectedError::QuicReadChunk(quinn::ReadError::ConnectionLost(_)) => true,
                        ConnectedError::QuicConnection(_) => true,
                        _ => false,
                    };

                    if connection_lost {
                        let err_str = e.to_string();
                        warn!(
                            "Connection lost during handshake to {}: {}",
                            target_addr, err_str
                        );

                        let user_msg = if err_str.contains("pairing mode")
                            || err_str.contains("Unknown client")
                        {
                            "Remote device rejected the connection — \
                             enable pairing mode on the remote device and try again."
                                .to_string()
                        } else {
                            format!("Connection lost during handshake: {}", err_str)
                        };

                        return Err(ConnectedError::PairingFailed(user_msg));
                    }

                    // Stream closed but connection still alive — this is expected
                    // when the receiver shows a pairing dialog.  The receiver will
                    // send trust confirmation on a NEW stream; poll periodically.
                    info!(
                        "Stream closed (receiver likely showing pairing dialog), \
                         waiting for trust confirmation: {}",
                        e
                    );
                }
            }

            // Stream closed - now poll periodically to check if trusted via background handler
            let check_interval = Duration::from_millis(500);
            let start = std::time::Instant::now();
            let pending_handshakes = self.pending_handshakes.clone();

            loop {
                // Wait before checking
                tokio::time::sleep(check_interval).await;

                // Check if we are still pending
                // If we are NOT pending, it means the background handler processed an Ack or Reject
                let is_pending = pending_handshakes.read().contains_key(&target_addr.ip());

                // Check if we've been trusted via background handler
                // We check trust first to handle the Ack case
                let current_fingerprint = transport.get_connection_fingerprint(target_addr).await;

                // Use captured fingerprint or current one
                let fp_to_check = current_fingerprint.or(peer_fingerprint.clone());

                if let Some(fp) = fp_to_check {
                    let ks = key_store.read();
                    if ks.is_trusted(&fp) {
                        // Get actual device info from trusted peers
                        let peer_info = ks
                            .get_trusted_peers()
                            .iter()
                            .find(|p| p.fingerprint == fp)
                            .cloned();
                        drop(ks);

                        let (dev_id, dev_name) = if let Some(info) = peer_info {
                            (
                                info.device_id.unwrap_or_else(|| "unknown".to_string()),
                                info.name.unwrap_or_else(|| "Unknown Device".to_string()),
                            )
                        } else {
                            ("already-trusted".to_string(), "Already Trusted".to_string())
                        };

                        info!("Peer trusted via background handler");
                        return Ok(Message::HandshakeAck {
                            device_id: dev_id,
                            device_name: dev_name,
                        });
                    }
                }

                if !is_pending {
                    // Not pending and not trusted -> Rejected
                    info!("Pending handshake cleared but not trusted - assuming Rejected");
                    return Err(ConnectedError::PairingFailed(
                        "Pairing rejected".to_string(),
                    ));
                }

                // Check if overall timeout exceeded
                if start.elapsed() >= HANDSHAKE_TIMEOUT - check_interval {
                    return Err(ConnectedError::Timeout("Handshake timeout".to_string()));
                }
            }
        })
        .await;

        send.finish().map_err(|e| ConnectedError::Io(e.into()))?;

        match ack_result {
            Ok(Ok(Message::HandshakeAck {
                device_id,
                device_name,
            })) => {
                // Emit DeviceFound to refresh UI with trusted status
                let device = Device::new(
                    device_id.clone(),
                    device_name.clone(),
                    addr.ip(),
                    addr.port(),
                    DeviceType::Unknown,
                );
                let _ = self.event_tx.send(ConnectedEvent::DeviceFound(device));

                // If already trusted via background handler, just return success
                if device_id == "already-trusted" {
                    info!("Pairing completed (via background trust confirmation)");
                    return Ok(());
                }

                // Use the fingerprint we captured at the start of the handshake
                // This is more reliable than trying to get it after the ack
                let fingerprint = if peer_fingerprint.is_some() {
                    peer_fingerprint.clone()
                } else {
                    // Fallback: try to get it again (might work if connection still open)
                    warn!("Using fallback fingerprint lookup for {}", addr);
                    self.transport.get_connection_fingerprint(addr).await
                };

                if let Some(fp) = fingerprint {
                    self.key_store.write().trust_peer(
                        fp,
                        Some(device_id.clone()),
                        Some(device_name.clone()),
                    )?;
                    info!("Pairing completed with {} ({})", device_name, device_id);

                    // Notify UI
                    let device = Device::new(
                        device_id,
                        device_name,
                        addr.ip(),
                        addr.port(),
                        DeviceType::Unknown,
                    );
                    let _ = self.event_tx.send(ConnectedEvent::DeviceFound(device));
                } else {
                    error!("Failed to get fingerprint for {}, trust not saved!", addr);
                    return Err(ConnectedError::PairingFailed(
                        "Could not obtain peer fingerprint".to_string(),
                    ));
                }
                Ok(())
            }
            Ok(Ok(_)) => Err(ConnectedError::PairingFailed(
                "Unexpected response".to_string(),
            )),
            Ok(Err(e)) => Err(ConnectedError::PairingFailed(e.to_string())),
            Err(_) => {
                // Final check - maybe trust was established just before timeout
                if let Some(fp) = self.transport.get_connection_fingerprint(addr).await
                    && self.key_store.read().is_trusted(&fp)
                {
                    info!("Pairing completed (trust established at timeout boundary)");
                    return Ok(());
                }
                Err(ConnectedError::Timeout("Handshake timeout".to_string()))
            }
        }
    }

    /// Send a HandshakeAck to confirm trust after user approves a pairing request.
    /// This is used instead of send_handshake when responding to an incoming pairing request.
    /// This way, the initiator receives an Ack (not a new Handshake) and both sides are paired.
    pub async fn send_trust_confirmation(&self, target_ip: IpAddr, target_port: u16) -> Result<()> {
        let addr = SocketAddr::new(target_ip, target_port);

        let (mut send, _recv) = self
            .transport
            .open_stream(addr, QuicTransport::STREAM_TYPE_CONTROL)
            .await?;

        let msg = Message::HandshakeAck {
            device_id: self.local_device.id.clone(),
            device_name: self.local_device.name.clone(),
        };

        let data = serde_json::to_vec(&msg)
            .map_err(|e| ConnectedError::InitializationError(e.to_string()))?;
        let len_bytes = (data.len() as u32).to_be_bytes();

        send.write_all(&len_bytes)
            .await
            .map_err(|e| ConnectedError::Io(e.into()))?;
        send.write_all(&data)
            .await
            .map_err(|e| ConnectedError::Io(e.into()))?;
        send.finish().map_err(|e| ConnectedError::Io(e.into()))?;

        info!("Sent trust confirmation to {}:{}", target_ip, target_port);
        Ok(())
    }

    /// Send unpair notification to a device
    pub async fn send_unpair_notification(
        &self,
        target_ip: IpAddr,
        target_port: u16,
        reason: UnpairReason,
    ) -> Result<()> {
        let addr = SocketAddr::new(target_ip, target_port);

        let (mut send, _recv) = match self
            .transport
            .open_stream(addr, QuicTransport::STREAM_TYPE_CONTROL)
            .await
        {
            Ok(s) => s,
            Err(e) => {
                warn!("Could not send unpair notification: {}", e);
                return Ok(()); // Don't fail the unpair if we can't notify
            }
        };

        let msg = Message::DeviceUnpaired {
            device_id: self.local_device.id.clone(),
            reason,
        };

        let data = serde_json::to_vec(&msg)
            .map_err(|e| ConnectedError::InitializationError(e.to_string()))?;
        let len_bytes = (data.len() as u32).to_be_bytes();

        let _ = send.write_all(&len_bytes).await;
        let _ = send.write_all(&data).await;
        let _ = send.finish();

        Ok(())
    }

    pub async fn send_clipboard(
        &self,
        target_ip: IpAddr,
        target_port: u16,
        text: String,
    ) -> Result<()> {
        let addr = SocketAddr::new(target_ip, target_port);

        // Try to send, retry once if connection fails
        let result = self.send_clipboard_inner(addr, text.clone()).await;

        if let Err(ref e) = result {
            let should_retry = matches!(
                e,
                ConnectedError::Timeout(_) | ConnectedError::Connection(_) | ConnectedError::Io(_)
            );

            if should_retry {
                info!(
                    "Clipboard send to {} failed ({}), invalidating connection and retrying...",
                    addr, e
                );
                self.transport.invalidate_connection(&addr);
                return self.send_clipboard_inner(addr, text).await;
            }
        }
        result
    }

    async fn send_clipboard_inner(&self, addr: SocketAddr, text: String) -> Result<()> {
        // No clipboard size limit — the user's clipboard can be arbitrarily large
        // (e.g. large code blocks, formatted text, etc.).  QUIC flow-control and
        // the per-message framing already handle backpressure on the wire.

        // Establish the connection / open a stream FIRST so that we have a live
        // TLS session from which we can extract the peer's certificate fingerprint.
        // Previously, the trust check ran before `open_stream`, which meant that
        // after a restart (empty connection cache) the fingerprint was always None
        // and every operation failed with PeerNotTrusted.
        let (mut send, _recv) = self
            .transport
            .open_stream(addr, QuicTransport::STREAM_TYPE_CONTROL)
            .await?;

        // M3: Verify the target is a trusted peer before sending sensitive clipboard data.
        // Now that the connection is established we can reliably read the fingerprint.
        let fingerprint = self.transport.get_connection_fingerprint(addr).await;
        match fingerprint {
            Some(ref fp) if self.key_store.read().is_trusted(fp) => {
                // Trusted — proceed
            }
            Some(ref fp) => {
                warn!(
                    "Refusing to send clipboard to untrusted peer {} (fingerprint: {})",
                    addr, fp
                );
                let _ = send.finish();
                return Err(ConnectedError::PeerNotTrusted);
            }
            None => {
                warn!(
                    "Refusing to send clipboard to {} — no TLS fingerprint available",
                    addr
                );
                let _ = send.finish();
                return Err(ConnectedError::PeerNotTrusted);
            }
        }

        let msg = Message::Clipboard { text };

        let data = serde_json::to_vec(&msg)
            .map_err(|e| ConnectedError::InitializationError(e.to_string()))?;
        let len_bytes = (data.len() as u32).to_be_bytes();

        send.write_all(&len_bytes)
            .await
            .map_err(|e| ConnectedError::Io(e.into()))?;
        send.write_all(&data)
            .await
            .map_err(|e| ConnectedError::Io(e.into()))?;
        send.finish().map_err(|e| ConnectedError::Io(e.into()))?;

        Ok(())
    }

    pub async fn send_media_control(
        &self,
        target_ip: IpAddr,
        target_port: u16,
        msg: crate::transport::MediaControlMessage,
    ) -> Result<()> {
        let addr = SocketAddr::new(target_ip, target_port);

        // Try to send, retry once if connection fails
        let result = self.send_media_control_inner(addr, msg.clone()).await;

        if let Err(ref e) = result {
            let should_retry = matches!(
                e,
                ConnectedError::Timeout(_) | ConnectedError::Connection(_) | ConnectedError::Io(_)
            );

            if should_retry {
                info!(
                    "Media control send to {} failed ({}), invalidating connection and retrying...",
                    addr, e
                );
                self.transport.invalidate_connection(&addr);
                return self.send_media_control_inner(addr, msg).await;
            }
        }
        result
    }

    async fn send_media_control_inner(
        &self,
        addr: SocketAddr,
        msg: crate::transport::MediaControlMessage,
    ) -> Result<()> {
        // Establish the connection first so the TLS fingerprint is available.
        let (mut send, _recv) = self
            .transport
            .open_stream(addr, QuicTransport::STREAM_TYPE_CONTROL)
            .await?;

        // M3: Verify the target is a trusted peer before sending media control data.
        let fingerprint = self.transport.get_connection_fingerprint(addr).await;
        match fingerprint {
            Some(ref fp) if self.key_store.read().is_trusted(fp) => {
                // Trusted — proceed
            }
            Some(ref fp) => {
                warn!(
                    "Refusing to send media control to untrusted peer {} (fingerprint: {})",
                    addr, fp
                );
                let _ = send.finish();
                return Err(ConnectedError::PeerNotTrusted);
            }
            None => {
                warn!(
                    "Refusing to send media control to {} — no TLS fingerprint available",
                    addr
                );
                let _ = send.finish();
                return Err(ConnectedError::PeerNotTrusted);
            }
        }

        let msg = Message::MediaControl(msg);

        let data = serde_json::to_vec(&msg)
            .map_err(|e| ConnectedError::InitializationError(e.to_string()))?;
        let len_bytes = (data.len() as u32).to_be_bytes();

        send.write_all(&len_bytes)
            .await
            .map_err(|e| ConnectedError::Io(e.into()))?;
        send.write_all(&data)
            .await
            .map_err(|e| ConnectedError::Io(e.into()))?;
        send.finish().map_err(|e| ConnectedError::Io(e.into()))?;

        Ok(())
    }

    pub async fn send_telephony(
        &self,
        target_ip: IpAddr,
        target_port: u16,
        msg: crate::telephony::TelephonyMessage,
    ) -> Result<()> {
        let addr = SocketAddr::new(target_ip, target_port);

        // Try to send, and if it fails due to connection issues, invalidate cache and retry once
        let result = self.send_telephony_inner(addr, &msg).await;

        if let Err(ref e) = result {
            // Check if this is a connection error that might be due to stale connection
            let should_retry = matches!(
                e,
                ConnectedError::Timeout(_) | ConnectedError::Connection(_) | ConnectedError::Io(_)
            );

            if should_retry {
                info!(
                    "Telephony send to {} failed ({}), invalidating connection and retrying...",
                    addr, e
                );
                self.transport.invalidate_connection(&addr);

                // Retry once with fresh connection
                return self.send_telephony_inner(addr, &msg).await;
            }
        }

        result
    }

    async fn send_telephony_inner(
        &self,
        addr: SocketAddr,
        msg: &crate::telephony::TelephonyMessage,
    ) -> Result<()> {
        // Establish the connection first so the TLS fingerprint is available.
        let (mut send, _recv) = self
            .transport
            .open_stream(addr, QuicTransport::STREAM_TYPE_CONTROL)
            .await?;

        // M3: Verify the target is a trusted peer before sending sensitive telephony data.
        let fingerprint = self.transport.get_connection_fingerprint(addr).await;
        match fingerprint {
            Some(ref fp) if self.key_store.read().is_trusted(fp) => {
                // Trusted — proceed
            }
            Some(ref fp) => {
                warn!(
                    "Refusing to send telephony data to untrusted peer {} (fingerprint: {})",
                    addr, fp
                );
                let _ = send.finish();
                return Err(ConnectedError::PeerNotTrusted);
            }
            None => {
                warn!(
                    "Refusing to send telephony data to {} — no TLS fingerprint available",
                    addr
                );
                let _ = send.finish();
                return Err(ConnectedError::PeerNotTrusted);
            }
        }

        let msg = Message::Telephony(msg.clone());

        let data = serde_json::to_vec(&msg)
            .map_err(|e| ConnectedError::InitializationError(e.to_string()))?;
        let len_bytes = (data.len() as u32).to_be_bytes();

        send.write_all(&len_bytes)
            .await
            .map_err(|e| ConnectedError::Io(e.into()))?;
        send.write_all(&data)
            .await
            .map_err(|e| ConnectedError::Io(e.into()))?;
        send.finish().map_err(|e| ConnectedError::Io(e.into()))?;

        Ok(())
    }

    pub async fn send_file(
        &self,
        target_ip: IpAddr,
        target_port: u16,
        file_path: PathBuf,
    ) -> Result<()> {
        if !file_path.exists() {
            return Err(ConnectedError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "File not found",
            )));
        }

        let addr = SocketAddr::new(target_ip, target_port);
        // Use connect_allow_unknown() so that files can be sent to discovered
        // but not-yet-paired devices.  The *receiver* side enforces the
        // accept/decline prompt for untrusted peers, so this is safe.
        let connection = self.transport.connect_allow_unknown(addr).await?;

        let event_tx = self.event_tx.clone();
        let transfer_id = uuid::Uuid::new_v4().to_string();
        let peer_name = target_ip.to_string(); // In real app, look up name from discovery

        tokio::spawn(async move {
            let (progress_tx, progress_rx) = mpsc::unbounded_channel();
            let mut progress_rx = progress_rx;

            // Bridge internal progress to public event bus
            let tid = transfer_id.clone();
            let p_peer = peer_name.clone();
            let p_tx = event_tx.clone();

            tokio::spawn(async move {
                while let Some(progress) = progress_rx.recv().await {
                    let event = match progress {
                        TransferProgress::Pending { .. } => {
                            // Pending is only used for incoming transfers, skip for outgoing
                            continue;
                        }
                        TransferProgress::CompressionProgress {
                            filename,
                            current_file,
                            files_processed,
                            total_files,
                            bytes_processed,
                            total_bytes,
                            speed_bytes_per_sec,
                        } => ConnectedEvent::CompressionProgress {
                            filename,
                            current_file,
                            files_processed,
                            total_files,
                            bytes_processed,
                            total_bytes,
                            speed_bytes_per_sec,
                        },
                        TransferProgress::Starting {
                            filename,
                            total_size,
                        } => ConnectedEvent::TransferStarting {
                            id: tid.clone(),
                            filename,
                            total_size,
                            peer_device: p_peer.clone(),
                            direction: TransferDirection::Outgoing,
                        },
                        TransferProgress::Progress {
                            bytes_transferred,
                            total_size,
                        } => ConnectedEvent::TransferProgress {
                            id: tid.clone(),
                            bytes_transferred,
                            total_size,
                        },
                        TransferProgress::Completed { filename, .. } => {
                            ConnectedEvent::TransferCompleted {
                                id: tid.clone(),
                                filename,
                            }
                        }
                        TransferProgress::Failed { error } => ConnectedEvent::TransferFailed {
                            id: tid.clone(),
                            error,
                        },
                        TransferProgress::Cancelled => ConnectedEvent::TransferFailed {
                            id: tid.clone(),
                            error: "Cancelled".into(),
                        },
                    };
                    let _ = p_tx.send(event);
                }
            });

            let file_transfer = FileTransfer::new(connection);
            if let Err(e) = file_transfer.send_file(&file_path, Some(progress_tx)).await {
                let _ = event_tx.send(ConnectedEvent::TransferFailed {
                    id: transfer_id,
                    error: e.to_string(),
                });
            }
        });

        Ok(())
    }

    /// Invalidate cached connection for a device by looking up its IP from discovered devices
    fn invalidate_connection_by_device_id(&self, device_id: &str) {
        self.invalidate_connection_by_device_id_with_reason(device_id, b"unpaired");
    }

    fn invalidate_connection_by_device_id_with_reason(&self, device_id: &str, reason: &[u8]) {
        let discovered = self.discovery.get_discovered_devices();
        info!(
            "Looking for device {} in {} discovered devices",
            device_id,
            discovered.len()
        );
        for d in &discovered {
            info!("  - discovered device: id={}, ip={}", d.id, d.ip);
        }

        if let Some(device) = discovered.iter().find(|d| d.id == device_id) {
            if let Some(ip) = device.ip_addr() {
                let addr = SocketAddr::new(ip, device.port);
                self.transport
                    .invalidate_connection_with_reason(&addr, reason);
                info!(
                    "Invalidated connection cache for device {} at {}",
                    device_id, addr
                );
            } else {
                warn!(
                    "Could not parse IP address '{}' for device {}",
                    device.ip, device_id
                );
            }
        } else {
            warn!(
                "Device {} not found in discovered devices, cannot invalidate connection",
                device_id
            );
        }
    }

    /// Check if an IP has a pending outgoing handshake
    pub fn has_pending_handshake(&self, ip: &IpAddr) -> bool {
        self.pending_handshakes.read().contains_key(ip)
    }

    /// Forget a device - completely removes trust.
    /// The device will need to go through full pairing request flow again.
    /// This is what the UI "Forget" action should call.
    pub async fn forget_device(&self, fingerprint: &str) -> Result<()> {
        // Explicitly disable pairing mode to prevent auto-reconnection loops
        self.set_pairing_mode(false);

        let (device_id, device_name, target) = {
            let ks = self.key_store.read();
            let info = ks.get_peer_info(fingerprint);
            if let Some(p) = info {
                let did = p.device_id.unwrap_or_else(|| "unknown".to_string());
                let name = p.name.unwrap_or_else(|| "Unknown".to_string());

                // Try to find IP/Port
                let target = self
                    .discovery
                    .get_device_by_id(&did)
                    .and_then(|d| d.ip_addr().map(|ip| (ip, d.port)));

                (did, name, target)
            } else {
                ("unknown".to_string(), "Unknown".to_string(), None)
            }
        };

        // Remove from keystore immediately to prevent auto-trust/reconnect during the notification delay
        self.key_store.write().remove_peer(fingerprint)?;

        if let Some((ip, _)) = target {
            self.pending_handshakes.write().remove(&ip);
        }

        // Try to send notification
        if let Some((ip, port)) = target {
            let _ = self
                .send_unpair_notification(ip, port, UnpairReason::Forgotten)
                .await;
        }

        // Invalidate cached connection
        self.invalidate_connection_by_device_id_with_reason(&device_id, b"forgotten");
        self.transport
            .invalidate_connection_by_fingerprint_with_reason(fingerprint, b"forgotten");

        // Emit DeviceUnpaired event
        let _ = self.event_tx.send(ConnectedEvent::DeviceUnpaired {
            device_id: device_id.clone(),
            device_name,
            reason: UnpairReason::Forgotten,
        });

        info!("Forgot device {} - trust completely removed", device_id);
        Ok(())
    }

    /// Forget a device by its device_id - completely removes trust.
    pub async fn forget_device_by_id(&self, device_id: &str) -> Result<()> {
        // Explicitly disable pairing mode to prevent auto-reconnection loops
        self.set_pairing_mode(false);

        let (fingerprint, device_name, target) = {
            let ks = self.key_store.read();
            let peers = ks.get_all_known_peers();
            if let Some(p) = peers
                .into_iter()
                .find(|p| p.device_id.as_deref() == Some(device_id))
            {
                let name = p.name.unwrap_or_else(|| "Unknown".to_string());

                // Try to find IP/Port
                let target = self
                    .discovery
                    .get_device_by_id(device_id)
                    .and_then(|d| d.ip_addr().map(|ip| (ip, d.port)));

                (Some(p.fingerprint), name, target)
            } else {
                (None, "Unknown".to_string(), None)
            }
        };

        if let Some(fp) = fingerprint {
            self.key_store.write().remove_peer(&fp)?;
            if let Some((ip, _)) = target {
                self.pending_handshakes.write().remove(&ip);
            }

            // Try to send notification
            if let Some((ip, port)) = target {
                let _ = self
                    .send_unpair_notification(ip, port, UnpairReason::Forgotten)
                    .await;
            }

            self.transport
                .invalidate_connection_by_fingerprint_with_reason(&fp, b"forgotten");
        } else {
            // Even if not found in keystore, try to invalidate connection
        }

        // Invalidate any cached connection to this device
        self.invalidate_connection_by_device_id_with_reason(device_id, b"forgotten");

        // Emit event so UI updates
        let _ = self.event_tx.send(ConnectedEvent::DeviceUnpaired {
            device_id: device_id.to_string(),
            device_name,
            reason: UnpairReason::Forgotten,
        });

        info!("Forgot device {} - trust completely removed", device_id);
        Ok(())
    }

    /// Completely remove a device from known peers.
    /// This allows re-pairing without any record (clean slate).
    ///
    /// Difference between forget and remove:
    /// - forget: Device must go through pairing request (user approval required)
    /// - remove: Device is unknown, can auto-pair in pairing mode
    /// - block: Device cannot connect at all
    pub fn is_device_forgotten(&self, fingerprint: &str) -> bool {
        self.key_store.read().is_forgotten(fingerprint)
    }

    pub fn get_forgotten_peers(&self) -> Vec<crate::security::PeerInfo> {
        self.key_store.read().get_forgotten_peers()
    }

    /// Unpair a device - disconnects but keeps trust intact.
    /// The device can reconnect automatically anytime (no re-pairing needed).
    /// This is what the UI "Unpair" action should call.
    pub async fn unpair_device(&self, fingerprint: &str) -> Result<()> {
        // Explicitly disable pairing mode to prevent auto-reconnection loops
        self.set_pairing_mode(false);

        let (device_id, device_name, target) = {
            let ks = self.key_store.read();
            let info = ks.get_peer_info(fingerprint);
            if let Some(p) = info {
                let did = p.device_id.unwrap_or_else(|| "unknown".to_string());
                let name = p.name.unwrap_or_else(|| "Unknown".to_string());

                // Try to find IP/Port
                let target = self
                    .discovery
                    .get_device_by_id(&did)
                    .and_then(|d| d.ip_addr().map(|ip| (ip, d.port)));

                (did, name, target)
            } else {
                ("unknown".to_string(), "Unknown".to_string(), None)
            }
        };

        // Mark as unpaired in keystore immediately
        if let Err(e) = self.key_store.write().unpair_peer(fingerprint.to_string()) {
            warn!("Failed to mark peer as unpaired in keystore: {}", e);
        }

        if let Some((ip, _)) = target {
            self.pending_handshakes.write().remove(&ip);
        }

        // Try to send notification
        if let Some((ip, port)) = target {
            let _ = self
                .send_unpair_notification(ip, port, UnpairReason::Unpaired)
                .await;
        }

        // Just invalidate the connection - trust remains intact
        self.invalidate_connection_by_device_id(&device_id);
        self.transport
            .invalidate_connection_by_fingerprint(fingerprint);

        // Emit event so UI updates
        let _ = self.event_tx.send(ConnectedEvent::DeviceUnpaired {
            device_id: device_id.clone(),
            device_name,
            reason: UnpairReason::Unpaired,
        });

        info!("Unpaired device - trust preserved, can reconnect anytime");
        Ok(())
    }

    /// Unpair a device by its device_id - disconnects but keeps trust intact.
    pub async fn unpair_device_by_id(&self, device_id: &str) -> Result<()> {
        // Explicitly disable pairing mode to prevent auto-reconnection loops
        self.set_pairing_mode(false);

        let (fingerprint, device_name, target) = {
            let ks = self.key_store.read();
            let peers = ks.get_all_known_peers();
            if let Some(p) = peers
                .into_iter()
                .find(|p| p.device_id.as_deref() == Some(device_id))
            {
                let name = p.name.unwrap_or_else(|| "Unknown".to_string());

                // Try to find IP/Port
                let target = self
                    .discovery
                    .get_device_by_id(device_id)
                    .and_then(|d| d.ip_addr().map(|ip| (ip, d.port)));

                (Some(p.fingerprint), name, target)
            } else {
                (None, "Unknown".to_string(), None)
            }
        };

        // Mark as unpaired in keystore immediately
        if let Some(fp) = &fingerprint
            && let Err(e) = self.key_store.write().unpair_peer(fp.clone())
        {
            warn!("Failed to mark peer as unpaired in keystore: {}", e);
        }

        if let Some((ip, _)) = target {
            self.pending_handshakes.write().remove(&ip);
        }

        if let Some((ip, port)) = target {
            let _ = self
                .send_unpair_notification(ip, port, UnpairReason::Unpaired)
                .await;
        }

        // Just invalidate the connection - trust remains intact
        self.invalidate_connection_by_device_id(device_id);
        if let Some(fp) = fingerprint {
            self.transport.invalidate_connection_by_fingerprint(&fp);
        }

        // Emit event so UI updates
        let _ = self.event_tx.send(ConnectedEvent::DeviceUnpaired {
            device_id: device_id.to_string(),
            device_name,
            reason: UnpairReason::Unpaired,
        });

        info!(
            "Unpaired device {} - trust preserved, can reconnect anytime",
            device_id
        );
        Ok(())
    }

    async fn start_background_tasks(&self) -> Result<()> {
        // 0. Periodic connection cache cleanup
        let transport_cleanup = self.transport.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            loop {
                interval.tick().await;
                transport_cleanup.cleanup_stale_connections();
            }
        });

        // 0b. Periodic cleanup of stale pending handshakes (remove entries older than 5 minutes)
        let pending_handshakes_cleanup = self.pending_handshakes.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                let cutoff = Instant::now() - Duration::from_secs(300);
                let mut hs = pending_handshakes_cleanup.write();
                let before = hs.len();
                hs.retain(|_, (ts, _)| *ts > cutoff);
                let removed = before - hs.len();
                if removed > 0 {
                    debug!("Cleaned up {} stale pending handshakes", removed);
                }
            }
        });

        // 1. Discovery Listener
        let (disco_tx, mut disco_rx) = mpsc::unbounded_channel();
        self.discovery
            .start_listening(disco_tx)
            .map_err(|e| ConnectedError::Discovery(e.to_string()))?;

        let event_tx = self.event_tx.clone();
        tokio::spawn(async move {
            while let Some(event) = disco_rx.recv().await {
                match event {
                    DiscoveryEvent::DeviceFound(d) => {
                        let _ = event_tx.send(ConnectedEvent::DeviceFound(d));
                    }
                    DiscoveryEvent::DeviceLost(id) => {
                        let _ = event_tx.send(ConnectedEvent::DeviceLost(id));
                    }
                    DiscoveryEvent::Error(msg) => {
                        let _ = event_tx
                            .send(ConnectedEvent::Error(format!("Discovery error: {}", msg)));
                    }
                }
            }
        });

        // 2. Transport Listener (Incoming Messages)
        let (msg_tx, mut msg_rx) = mpsc::unbounded_channel();
        let (file_tx, mut file_rx) = mpsc::unbounded_channel();
        let (fs_tx, mut fs_rx) = mpsc::unbounded_channel();

        self.transport.start_server(msg_tx, file_tx, fs_tx).await?;

        let event_tx = self.event_tx.clone();
        let key_store_control = self.key_store.clone();
        let local_id = self.local_device.id.clone();
        let local_name = self.local_device.name.clone();
        let pending_handshakes = self.pending_handshakes.clone();
        let discovery = self.discovery.clone();
        let transport = self.transport.clone();

        // Handle Control Messages
        tokio::spawn(async move {
            let key_store = key_store_control;
            while let Some((addr, fingerprint, msg, send_stream)) = msg_rx.recv().await {
                match msg {
                    Message::Handshake {
                        device_id,
                        device_name,
                        listening_port,
                    } => {
                        let mut send_stream = send_stream;
                        let (resolved_name, resolved_type) = discovery
                            .get_device_by_id(&device_id)
                            .map(|d| (d.name, d.device_type))
                            .unwrap_or_else(|| (device_name.clone(), DeviceType::Unknown));

                        let device = Device::new(
                            device_id.clone(),
                            resolved_name,
                            addr.ip(),
                            listening_port,
                            resolved_type,
                        );

                        if let Some(event) = discovery
                            .upsert_device_endpoint(device.clone(), DiscoverySource::Connected)
                        {
                            match event {
                                DiscoveryEvent::DeviceFound(d) => {
                                    let _ = event_tx.send(ConnectedEvent::DeviceFound(d));
                                }
                                DiscoveryEvent::DeviceLost(id) => {
                                    let _ = event_tx.send(ConnectedEvent::DeviceLost(id));
                                }
                                DiscoveryEvent::Error(msg) => {
                                    let _ = event_tx.send(ConnectedEvent::Error(format!(
                                        "Discovery error: {}",
                                        msg
                                    )));
                                }
                            }
                        }

                        // Register alias for the active connection if port differs
                        if listening_port != 0 && listening_port != addr.port() {
                            let listening_addr = SocketAddr::new(addr.ip(), listening_port);
                            transport.register_connection_alias(addr, listening_addr);
                        }

                        // Take a single KeyStore snapshot to avoid inconsistent state
                        // between multiple separate reads (TOCTOU within the lock).
                        let (is_trusted, is_forgotten, is_unpaired, _needs_pairing) = {
                            let ks = key_store.read();
                            (
                                ks.is_trusted(&fingerprint),
                                ks.is_forgotten(&fingerprint),
                                ks.is_unpaired(&fingerprint),
                                ks.needs_pairing_request(&fingerprint),
                            )
                        };
                        // Retrieve the full pending entry (timestamp + expected fingerprint)
                        let pending_entry = pending_handshakes.read().get(&addr.ip()).cloned();
                        let has_pending = pending_entry.is_some();

                        // If forgotten, always require re-pairing approval (don't auto-trust)
                        if is_forgotten {
                            info!(
                                "Received Handshake from FORGOTTEN peer (requires re-approval): {} - {}",
                                device_name, fingerprint
                            );
                            let _ = event_tx.send(ConnectedEvent::PairingRequest {
                                fingerprint: fingerprint.clone(),
                                device_name,
                                device_id,
                            });
                            continue;
                        }

                        // If we have a pending handshake with a stored fingerprint,
                        // verify it matches this connection's fingerprint to prevent
                        // auto-trusting a different device that shares/spoofs the IP.
                        let pending_fp_matches = match &pending_entry {
                            Some((_, Some(expected_fp))) => *expected_fp == fingerprint,
                            Some((_, None)) => true, // No fingerprint stored yet — allow
                            None => false,
                        };

                        if (!is_trusted && has_pending && pending_fp_matches) || is_unpaired {
                            info!(
                                "Auto-trusting peer from Handshake (pending outbound pairing or unpaired): {} - {}",
                                device_name, fingerprint
                            );
                            pending_handshakes.write().remove(&addr.ip());

                            if let Err(e) = key_store.write().trust_peer(
                                fingerprint.clone(),
                                Some(device_id.clone()),
                                Some(device_name.clone()),
                            ) {
                                error!("Failed to auto-trust peer: {}", e);
                            }

                            // Emit DeviceFound to refresh UI
                            let d = Device::new(
                                device_id,
                                device_name.clone(),
                                addr.ip(),
                                addr.port(),
                                DeviceType::Unknown,
                            );
                            let _ = discovery
                                .upsert_device_endpoint(d.clone(), DiscoverySource::Connected);
                            let _ = event_tx.send(ConnectedEvent::DeviceFound(d));

                            if let Some(mut send) = send_stream.take() {
                                let msg = Message::HandshakeAck {
                                    device_id: local_id.clone(),
                                    device_name: local_name.clone(),
                                };
                                if let Ok(data) = serde_json::to_vec(&msg) {
                                    let len_bytes = (data.len() as u32).to_be_bytes();
                                    if send.write_all(&len_bytes).await.is_ok() {
                                        let _ = send.write_all(&data).await;
                                    }
                                    let _ = send.finish();
                                }
                            }

                            continue;
                        }

                        if !is_trusted {
                            // NEVER auto-trust incoming Handshakes - always require user approval
                            // This is a security measure: just because we're in pairing mode
                            // doesn't mean we should auto-trust anyone who connects to us.
                            // Pairing mode only affects OUR outgoing handshakes being auto-trusted
                            // by the remote side when we receive their HandshakeAck.
                            info!(
                                "Received Handshake from untrusted peer: {} - {} (showing pairing request)",
                                device_name, fingerprint
                            );
                            let _ = event_tx.send(ConnectedEvent::PairingRequest {
                                fingerprint: fingerprint.clone(),
                                device_name,
                                device_id,
                            });
                            // Don't send any response - the trust confirmation will be sent
                            // on a new stream when the user trusts via the UI
                        } else {
                            debug!("Received Handshake from trusted peer: {}", device_name);
                            // Update device_id for this trusted peer if it's missing or changed
                            if let Err(e) = key_store.write().trust_peer(
                                fingerprint.clone(),
                                Some(device_id.clone()),
                                Some(device_name.clone()),
                            ) {
                                error!("Failed to update trusted peer info: {}", e);
                            }

                            // Emit DeviceFound to refresh UI with trusted status
                            let device = Device::new(
                                device_id.clone(),
                                device_name.clone(),
                                addr.ip(),
                                addr.port(),
                                DeviceType::Unknown,
                            );
                            let _ = event_tx.send(ConnectedEvent::DeviceFound(device));

                            // ALSO emit PairingRequest even if trusted.
                            // This signals to the UI that an *active connection attempt* is happening,
                            // allowing it to clear any "locally unpaired" / hidden state if necessary.
                            // The UI should auto-accept this if the device is already trusted.
                            let _ = event_tx.send(ConnectedEvent::PairingRequest {
                                fingerprint: fingerprint.clone(),
                                device_name: device_name.clone(),
                                device_id: device_id.clone(),
                            });

                            // Register alias for the active connection if port differs
                            if listening_port != 0 && listening_port != addr.port() {
                                let listening_addr = SocketAddr::new(addr.ip(), listening_port);
                                transport.register_connection_alias(addr, listening_addr);
                            }

                            if let Some(mut send) = send_stream.take() {
                                let msg = Message::HandshakeAck {
                                    device_id: local_id.clone(),
                                    device_name: local_name.clone(),
                                };
                                if let Ok(data) = serde_json::to_vec(&msg) {
                                    let len_bytes = (data.len() as u32).to_be_bytes();
                                    if send.write_all(&len_bytes).await.is_ok() {
                                        let _ = send.write_all(&data).await;
                                    }
                                    let _ = send.finish();
                                }
                            }
                        }
                    }
                    Message::HandshakeAck {
                        device_id: remote_device_id,
                        device_name,
                    } => {
                        info!("Received HandshakeAck from {}", device_name);

                        // Take a single KeyStore snapshot to avoid inconsistent state
                        let (is_trusted, is_forgotten) = {
                            let ks = key_store.read();
                            (ks.is_trusted(&fingerprint), ks.is_forgotten(&fingerprint))
                        };
                        let pending_entry = pending_handshakes.read().get(&addr.ip()).cloned();
                        let has_pending = pending_entry.is_some();

                        // If forgotten, require re-approval even for acks
                        if is_forgotten {
                            info!(
                                "Received HandshakeAck from FORGOTTEN peer (requires re-approval): {} - {}",
                                device_name, fingerprint
                            );
                            let _ = event_tx.send(ConnectedEvent::PairingRequest {
                                fingerprint: fingerprint.clone(),
                                device_name,
                                device_id: remote_device_id,
                            });
                            continue;
                        }

                        if !is_trusted {
                            // Only auto-trust if we have a pending outbound handshake to this IP
                            // (i.e. WE initiated the pairing). Never auto-trust just because
                            // pairing mode is enabled — that would allow an attacker to send an
                            // unsolicited HandshakeAck and get trusted without user approval.
                            // Verify the fingerprint matches our pending handshake
                            // to prevent a different device from being auto-trusted.
                            let pending_fp_matches = match &pending_entry {
                                Some((_, Some(expected_fp))) => *expected_fp == fingerprint,
                                Some((_, None)) => true, // No fingerprint stored yet — allow
                                None => false,
                            };

                            if has_pending && pending_fp_matches {
                                info!(
                                    "Auto-trusting peer from HandshakeAck: {} - {} (pending outbound handshake)",
                                    device_name, fingerprint,
                                );

                                // Remove from pending handshakes
                                pending_handshakes.write().remove(&addr.ip());

                                if let Err(e) = key_store.write().trust_peer(
                                    fingerprint.clone(),
                                    Some(remote_device_id.clone()),
                                    Some(device_name.clone()),
                                ) {
                                    error!("Failed to auto-trust peer: {}", e);
                                }

                                // Emit DeviceFound to refresh UI
                                use crate::device::DeviceType;
                                let d = Device::new(
                                    remote_device_id,
                                    device_name,
                                    addr.ip(),
                                    addr.port(),
                                    DeviceType::Unknown,
                                );
                                // Update discovery with trusted connection info
                                let _ = discovery
                                    .upsert_device_endpoint(d.clone(), DiscoverySource::Connected);
                                let _ = event_tx.send(ConnectedEvent::DeviceFound(d));
                            } else {
                                warn!(
                                    "Received unsolicited HandshakeAck from {} ({}) - ignoring (no pending outbound handshake)",
                                    device_name, fingerprint
                                );
                            }
                        } else {
                            if let Err(e) = key_store.write().trust_peer(
                                fingerprint.clone(),
                                Some(remote_device_id.clone()),
                                Some(device_name.clone()),
                            ) {
                                error!("Failed to update trusted peer info on Ack: {}", e);
                            }

                            use crate::device::DeviceType;
                            let d = Device::new(
                                remote_device_id,
                                device_name,
                                addr.ip(),
                                addr.port(),
                                DeviceType::Unknown,
                            );
                            // Update discovery with trusted connection info
                            let _ = discovery
                                .upsert_device_endpoint(d.clone(), DiscoverySource::Connected);
                            let _ = event_tx.send(ConnectedEvent::DeviceFound(d));
                        }
                    }
                    Message::HandshakeReject { device_id, reason } => {
                        let device_name = key_store
                            .read()
                            .get_peer_name(&fingerprint)
                            .unwrap_or_else(|| device_id.clone());

                        info!(
                            "Received HandshakeReject from {} ({}): {:?}",
                            device_name, device_id, reason
                        );

                        pending_handshakes.write().remove(&addr.ip());

                        // Invalidate connection to ensure clean state
                        transport.invalidate_connection(&addr);

                        let _ = event_tx.send(ConnectedEvent::PairingRejected {
                            device_name,
                            device_id,
                        });
                    }
                    Message::Clipboard { text } => {
                        // No clipboard size limit — the user's clipboard can be
                        // arbitrarily large (e.g. large code blocks, formatted text).
                        // Trust verification below ensures only trusted peers can
                        // send clipboard data, and QUIC flow-control handles backpressure.

                        // Single KeyStore snapshot for trust check + name lookup
                        let (is_trusted, from_device) = {
                            let ks = key_store.read();
                            let trusted = ks.is_trusted(&fingerprint);
                            let name = ks
                                .get_peer_name(&fingerprint)
                                .unwrap_or_else(|| "Unknown".to_string());
                            (trusted, name)
                        };
                        if is_trusted {
                            let _ = event_tx.send(ConnectedEvent::ClipboardReceived {
                                content: text,
                                from_device,
                            });
                        } else {
                            warn!(
                                "Ignored clipboard from untrusted peer {} at {}",
                                fingerprint, addr
                            );
                        }
                    }
                    Message::MediaControl(media_msg) => {
                        // Single KeyStore snapshot for trust check + name lookup
                        let (is_trusted, from_device) = {
                            let ks = key_store.read();
                            let trusted = ks.is_trusted(&fingerprint);
                            let name = ks
                                .get_peer_name(&fingerprint)
                                .unwrap_or_else(|| "Unknown".to_string());
                            (trusted, name)
                        };
                        if is_trusted {
                            let _ = event_tx.send(ConnectedEvent::MediaControl {
                                from_device,
                                event: media_msg,
                            });
                        }
                    }
                    Message::Telephony(telephony_msg) => {
                        // Single KeyStore snapshot for trust check + name lookup
                        let (is_trusted, from_device) = {
                            let ks = key_store.read();
                            let trusted = ks.is_trusted(&fingerprint);
                            let name = ks
                                .get_peer_name(&fingerprint)
                                .unwrap_or_else(|| "Unknown".to_string());
                            (trusted, name)
                        };
                        if is_trusted {
                            let _ = event_tx.send(ConnectedEvent::Telephony {
                                from_device,
                                from_ip: addr.ip().to_string(),
                                from_port: addr.port(),
                                message: telephony_msg,
                            });
                        }
                    }
                    Message::DeviceUnpaired {
                        device_id: claimed_device_id,
                        reason,
                    } => {
                        // Security fix (#2): Do NOT trust the sender's claimed device_id.
                        // The TLS fingerprint is the only authenticated identity — use it
                        // to look up the real device_id from our keystore.  A malicious
                        // peer could send any device_id in the message to try to unpair
                        // a different device on our side.
                        let (resolved_device_id, device_name) = {
                            let ks = key_store.read();
                            match ks.get_peer_info(&fingerprint) {
                                Some(info) => {
                                    let did = info
                                        .device_id
                                        .clone()
                                        .unwrap_or_else(|| claimed_device_id.clone());
                                    let name = info.name.clone().unwrap_or_else(|| did.clone());

                                    if info.device_id.as_deref() != Some(&claimed_device_id) {
                                        warn!(
                                            "DeviceUnpaired: sender claimed device_id '{}' but \
                                             fingerprint {} maps to device_id '{}'.  Using \
                                             authenticated id.",
                                            claimed_device_id, fingerprint, did
                                        );
                                    }
                                    (did, name)
                                }
                                None => {
                                    warn!(
                                        "DeviceUnpaired from unknown fingerprint {} (claimed {})",
                                        fingerprint, claimed_device_id
                                    );
                                    (claimed_device_id.clone(), claimed_device_id.clone())
                                }
                            }
                        };

                        match reason {
                            UnpairReason::Unpaired => {
                                info!("Device {} unpaired (trust preserved)", resolved_device_id);
                                if let Err(e) = key_store.write().unpair_peer(fingerprint.clone()) {
                                    error!("Failed to mark peer as unpaired: {}", e);
                                }
                            }
                            UnpairReason::Forgotten => {
                                if let Err(e) = key_store.write().remove_peer(&fingerprint) {
                                    error!("Failed to remove unpaired peer: {}", e);
                                }
                            }
                        }

                        // Invalidate connection to ensure clean state on both sides
                        transport.invalidate_connection(&addr);

                        let _ = event_tx.send(ConnectedEvent::DeviceUnpaired {
                            device_id: resolved_device_id,
                            device_name,
                            reason,
                        });
                    }

                    _ => {}
                }
            }
        });

        // Handle Filesystem Streams
        let fs_provider_clone = self.fs_provider.clone();
        let key_store_fs = self.key_store.clone();
        let fs_semaphore = self.fs_stream_semaphore.clone();

        tokio::spawn(async move {
            while let Some((fingerprint, mut send, mut recv)) = fs_rx.recv().await {
                let is_trusted = key_store_fs.read().is_trusted(&fingerprint);
                if !is_trusted {
                    error!(
                        "Rejected Filesystem Stream from untrusted peer: {}",
                        fingerprint
                    );
                    continue;
                }

                let provider_ref = fs_provider_clone.clone();
                let sem = fs_semaphore.clone();

                tokio::spawn(async move {
                    // Acquire a permit from the global FS-stream semaphore to
                    // bound the number of concurrent handlers (and thus memory).
                    let _permit = match sem.acquire().await {
                        Ok(permit) => permit,
                        Err(_) => {
                            warn!(
                                "FS stream semaphore closed, dropping stream from {}",
                                fingerprint
                            );
                            return;
                        }
                    };
                    use crate::file_transfer::send_message;
                    use crate::filesystem::FilesystemMessage;

                    loop {
                        let msg: Result<FilesystemMessage> =
                            crate::file_transfer::recv_message_with_limit(
                                &mut recv,
                                8 * 1024 * 1024,
                            )
                            .await;
                        match msg {
                            Ok(req) => {
                                let provider = provider_ref.clone();
                                let response = tokio::task::spawn_blocking(move || {
                                    let lock = provider.read();
                                    if let Some(p) = lock.as_ref() {
                                        match req {
                                            FilesystemMessage::ListDirRequest { path } => {
                                                match p.list_dir(&path) {
                                                    Ok(entries) => {
                                                        FilesystemMessage::ListDirResponse {
                                                            entries,
                                                        }
                                                    }
                                                    Err(e) => FilesystemMessage::Error {
                                                        message: e.to_string(),
                                                    },
                                                }
                                            }
                                            FilesystemMessage::ReadFileRequest {
                                                path,
                                                offset,
                                                size,
                                            } => match p.read_file(&path, offset, size) {
                                                Ok(data) => {
                                                    FilesystemMessage::ReadFileResponse { data }
                                                }
                                                Err(e) => FilesystemMessage::Error {
                                                    message: e.to_string(),
                                                },
                                            },
                                            FilesystemMessage::WriteFileRequest {
                                                path,
                                                offset: _,
                                                data: _,
                                            } => {
                                                // C4: Remote writes under $HOME are too dangerous.
                                                // A trusted peer could overwrite ~/.ssh/authorized_keys,
                                                // ~/.bashrc, crontabs, etc. Reject all remote writes
                                                // until an explicit allowlist / user-confirmation flow
                                                // is implemented.
                                                warn!(
                                                    "Rejected WriteFileRequest for '{}' — remote writes are disabled for security",
                                                    path
                                                );
                                                FilesystemMessage::Error {
                                                    message: "Remote file writes are disabled for security reasons".to_string(),
                                                }
                                            }
                                            FilesystemMessage::GetMetadataRequest { path } => {
                                                match p.get_metadata(&path) {
                                                    Ok(entry) => {
                                                        FilesystemMessage::GetMetadataResponse {
                                                            entry,
                                                        }
                                                    }
                                                    Err(e) => FilesystemMessage::Error {
                                                        message: e.to_string(),
                                                    },
                                                }
                                            }
                                            FilesystemMessage::GetThumbnailRequest { path } => {
                                                match p.get_thumbnail(&path) {
                                                    Ok(data) => {
                                                        FilesystemMessage::GetThumbnailResponse {
                                                            data,
                                                        }
                                                    }
                                                    Err(e) => FilesystemMessage::Error {
                                                        message: e.to_string(),
                                                    },
                                                }
                                            }
                                            _ => FilesystemMessage::Error {
                                                message: "Not implemented".to_string(),
                                            },
                                        }
                                    } else {
                                        FilesystemMessage::Error {
                                            message: "No filesystem provider registered"
                                                .to_string(),
                                        }
                                    }
                                })
                                .await;

                                match response {
                                    Ok(resp_msg) => {
                                        if let Err(e) = send_message(&mut send, &resp_msg).await {
                                            warn!("Failed to send FS response: {}", e);
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        error!("FS Handler panicked: {}", e);
                                        break;
                                    }
                                }
                            }
                            Err(_) => break, // Stream closed
                        }
                    }
                });
            }
        });

        // Handle File Streams
        let event_tx = self.event_tx.clone();
        let download_dir_lock = self.download_dir.clone();
        let key_store_files = self.key_store.clone();

        let pending_transfers = self.pending_transfers.clone();
        tokio::spawn(async move {
            let key_store = key_store_files;
            while let Some((fingerprint, send, recv)) = file_rx.recv().await {
                let is_trusted = key_store.read().is_trusted(&fingerprint);

                // Always reject blocked peers
                if key_store.read().is_blocked(&fingerprint) {
                    error!("Rejected File Stream from blocked peer: {}", fingerprint);
                    continue;
                }

                let event_tx = event_tx.clone();
                let download_dir = download_dir_lock.read().clone();
                let fingerprint_clone = fingerprint.clone();
                // Paired (trusted) devices auto-accept file transfers;
                // unpaired devices get an accept/decline prompt.
                let should_auto_accept = is_trusted;
                let pending = pending_transfers.clone();
                let peer_name = key_store
                    .read()
                    .get_peer_name(&fingerprint)
                    .unwrap_or_else(|| "Unknown device".to_string());

                tokio::spawn(async move {
                    let transfer_id = uuid::Uuid::new_v4().to_string();
                    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();

                    let tid = transfer_id.clone();
                    let p_peer = peer_name.clone();
                    let p_fingerprint = fingerprint_clone.clone();
                    let p_tx = event_tx.clone();

                    // Bridge progress events
                    tokio::spawn(async move {
                        while let Some(p) = progress_rx.recv().await {
                            match p {
                                TransferProgress::Pending {
                                    filename,
                                    total_size,
                                    ..
                                } => {
                                    // Emit TransferRequest for user approval
                                    let _ = p_tx.send(ConnectedEvent::TransferRequest {
                                        id: tid.clone(),
                                        filename,
                                        size: total_size,
                                        from_device: p_peer.clone(),
                                        from_fingerprint: p_fingerprint.clone(),
                                    });
                                }
                                TransferProgress::Starting {
                                    filename,
                                    total_size,
                                } => {
                                    let _ = p_tx.send(ConnectedEvent::TransferStarting {
                                        id: tid.clone(),
                                        filename,
                                        total_size,
                                        peer_device: p_peer.clone(),
                                        direction: TransferDirection::Incoming,
                                    });
                                }
                                TransferProgress::Progress {
                                    bytes_transferred,
                                    total_size,
                                } => {
                                    let _ = p_tx.send(ConnectedEvent::TransferProgress {
                                        id: tid.clone(),
                                        bytes_transferred,
                                        total_size,
                                    });
                                }
                                TransferProgress::Completed { filename, .. } => {
                                    let _ = p_tx.send(ConnectedEvent::TransferCompleted {
                                        id: tid.clone(),
                                        filename,
                                    });
                                }
                                TransferProgress::Failed { error } => {
                                    let _ = p_tx.send(ConnectedEvent::TransferFailed {
                                        id: tid.clone(),
                                        error,
                                    });
                                }
                                TransferProgress::CompressionProgress {
                                    filename,
                                    current_file,
                                    files_processed,
                                    total_files,
                                    bytes_processed,
                                    total_bytes,
                                    speed_bytes_per_sec,
                                } => {
                                    let _ = p_tx.send(ConnectedEvent::CompressionProgress {
                                        filename,
                                        current_file,
                                        files_processed,
                                        total_files,
                                        bytes_processed,
                                        total_bytes,
                                        speed_bytes_per_sec,
                                    });
                                }
                                TransferProgress::Cancelled => {
                                    let _ = p_tx.send(ConnectedEvent::TransferFailed {
                                        id: tid.clone(),
                                        error: "Cancelled".into(),
                                    });
                                }
                            }
                        }
                    });

                    // Create accept channel for user confirmation if not auto-accepting
                    let accept_rx = if !should_auto_accept {
                        let (tx, rx) = oneshot::channel();
                        pending.write().insert(transfer_id.clone(), tx);
                        Some(rx)
                    } else {
                        None
                    };

                    // Handle incoming file with auto-accept preference
                    let result = FileTransfer::handle_incoming(
                        send,
                        recv,
                        download_dir,
                        Some(progress_tx),
                        should_auto_accept,
                        accept_rx,
                    )
                    .await;

                    // H3: Always clean up the pending transfer entry regardless of
                    // success/failure/timeout to prevent stale entries from accumulating.
                    pending.write().remove(&transfer_id);

                    if let Err(e) = result {
                        error!("File receive failed: {}", e);
                        let _ = event_tx.send(ConnectedEvent::TransferFailed {
                            id: transfer_id,
                            error: e.to_string(),
                        });
                    }
                });
            }
        });

        Ok(())
    }
}

impl ConnectedClient {
    pub async fn shutdown(&self) {
        info!("Shutting down ConnectedClient...");

        // Cancel pairing mode timeout if active
        if let Some(handle) = self.pairing_mode_handle.write().take() {
            handle.abort();
        }

        // Shutdown transport (closes connections)
        self.transport.shutdown().await;

        // Shutdown discovery (unregisters mDNS, stops threads)
        self.discovery.shutdown();

        info!("ConnectedClient shutdown complete");
    }

    pub async fn broadcast_clipboard(&self, text: String) -> Result<usize> {
        let devices = self.get_discovered_devices();

        let mut tasks = Vec::new();
        let transport = self.transport.clone();
        let key_store = self.key_store.clone();

        for device in devices {
            if let Some(ip) = device.ip_addr() {
                let port = device.port;
                let txt = text.clone();
                let t = transport.clone();
                let ks = key_store.clone();

                tasks.push(tokio::spawn(async move {
                    let addr = SocketAddr::new(ip, port);

                    // Establish the connection / open a stream FIRST so the TLS
                    // fingerprint is available for the trust check. Without a live
                    // connection (e.g. after restart) the cache is empty and the
                    // fingerprint would be None, skipping every device.
                    let (mut send, _recv) = match t
                        .open_stream(addr, QuicTransport::STREAM_TYPE_CONTROL)
                        .await
                    {
                        Ok(stream) => stream,
                        Err(e) => {
                            debug!("Failed to open stream to {}: {}", addr, e);
                            return false;
                        }
                    };

                    // Verify trust via TLS certificate fingerprint (not spoofable mDNS device IDs)
                    let fingerprint = t.get_connection_fingerprint(addr).await;
                    match fingerprint {
                        Some(ref fp) if ks.read().is_trusted(fp) => {
                            // Trusted via TLS fingerprint — safe to send
                        }
                        Some(ref fp) => {
                            debug!(
                                "Skipping broadcast to {} — untrusted fingerprint {}",
                                addr, fp
                            );
                            let _ = send.finish();
                            return false;
                        }
                        None => {
                            debug!(
                                "Skipping broadcast to {} — no TLS fingerprint available",
                                addr
                            );
                            let _ = send.finish();
                            return false;
                        }
                    }

                    let msg = Message::Clipboard { text: txt };
                    if let Ok(data) = serde_json::to_vec(&msg) {
                        let len_bytes = (data.len() as u32).to_be_bytes();
                        let _ = send.write_all(&len_bytes).await;
                        let _ = send.write_all(&data).await;
                        let _ = send.finish();
                        return true;
                    }
                    false
                }));
            }
        }

        let mut sent_count = 0;
        for task in tasks {
            if let Ok(success) = task.await
                && success
            {
                sent_count += 1;
            }
        }
        Ok(sent_count)
    }
}

fn get_local_ip() -> Option<IpAddr> {
    let ifaces = if_addrs::get_if_addrs().ok()?;

    // Filter for IPv4 and non-loopback
    let ipv4_ifaces: Vec<_> = ifaces
        .into_iter()
        .filter(|iface| {
            !iface.is_loopback()
                && iface.ip().is_ipv4()
                && !iface.ip().to_string().starts_with("127.")
        })
        .collect();

    // Priority 1: Physical-looking interfaces (wlan, eth, en, wi-fi)
    // EXCLUDING virtual adapters (vethernet, wsl, virtual, etc.)
    let priority_names = ["wlan", "eth", "en", "wi-fi", "ethernet"];
    let virtual_names = [
        "vethernet",
        "wsl",
        "virtual",
        "vbox",
        "vmware",
        "docker",
        "tap",
        "tun",
        "br-",
        "wg",
        "nordlynx",
        "proton",
        "mullvad",
        "wireguard",
    ];

    ipv4_ifaces
        .iter()
        .find(|iface| {
            let name = iface.name.to_lowercase();
            let is_virtual = virtual_names.iter().any(|v| name.contains(v));
            let is_priority = priority_names.iter().any(|p| name.contains(p));
            is_priority && !is_virtual
        })
        .or_else(|| {
            // Priority 2: 192.168.x.x (Most common home LAN)
            ipv4_ifaces.iter().find(|iface| {
                if let IpAddr::V4(ipv4) = iface.ip() {
                    let octets = ipv4.octets();
                    octets[0] == 192 && octets[1] == 168
                } else {
                    false
                }
            })
        })
        .or_else(|| {
            // Priority 3: 10.x.x.x (Enterprise LAN, but also some VPNs)
            ipv4_ifaces.iter().find(|iface| {
                if let IpAddr::V4(ipv4) = iface.ip() {
                    let octets = ipv4.octets();
                    octets[0] == 10
                } else {
                    false
                }
            })
        })
        .or_else(|| {
            // Priority 4: 172.16-31.x.x (Often Docker/WSL, so lower priority)
            ipv4_ifaces.iter().find(|iface| {
                if let IpAddr::V4(ipv4) = iface.ip() {
                    let octets = ipv4.octets();
                    octets[0] == 172 && octets[1] >= 16 && octets[1] <= 31
                } else {
                    false
                }
            })
        })
        .map(|iface| iface.ip())
        .or_else(|| {
            // Fallback to any IPv4
            ipv4_ifaces.first().map(|iface| iface.ip())
        })
}
