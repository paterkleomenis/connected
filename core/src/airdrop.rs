use crate::device::{Device, DeviceType};
use crate::error::{ConnectedError, Result};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use mdns_sd::{ServiceDaemon, ServiceInfo};
use parking_lot::RwLock;
use rcgen::{CertificateParams, KeyPair};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;
use tracing::{debug, error, info, warn};

// ============================================================================
// Helpers
// ============================================================================

/// Serialize a value to a binary plist byte vector.
fn serialize_plist_binary<T: Serialize>(value: &T) -> Vec<u8> {
    let mut buf = Vec::new();
    plist::to_writer_binary(&mut buf, value).unwrap_or_default();
    buf
}

/// Serialize a value to an XML plist byte vector.
#[allow(dead_code)]
fn serialize_plist_xml<T: Serialize>(value: &T) -> Vec<u8> {
    let mut buf = Vec::new();
    plist::to_writer_xml(&mut buf, value).unwrap_or_default();
    buf
}

// ============================================================================
// AirDrop Protocol Constants
// ============================================================================

/// AirDrop mDNS service type (Bonjour)
const AIRDROP_SERVICE_TYPE: &str = "_airdrop._tcp.local.";

// ============================================================================
// Data Structures
// ============================================================================

/// AirDrop service information broadcast via mDNS
#[derive(Debug, Clone)]
pub struct AirDropServiceInfo {
    pub device_id: String,
    pub device_name: String,
    pub port: u16,
    pub device_type: DeviceType,
    pub discoverable_everyone: bool,
}

/// AirDrop Discover request (sent by sender)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirDropDiscoverRequest {
    #[serde(rename = "SenderRecordData")]
    pub sender_record_data: Vec<u8>,
}

/// AirDrop Discover response (sent by receiver)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirDropDiscoverResponse {
    #[serde(rename = "ReceiverComputerName")]
    pub receiver_computer_name: String,
    #[serde(rename = "ReceiverMediaCapabilities")]
    pub receiver_media_capabilities: String,
    #[serde(rename = "ReceiverModelName")]
    pub receiver_model_name: String,
}

/// AirDrop Ask request (sender asks to send a file)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirDropAskRequest {
    #[serde(rename = "SenderComputerName")]
    pub sender_computer_name: String,
    #[serde(rename = "BundleID")]
    pub bundle_id: String,
    #[serde(rename = "Files")]
    pub files: Vec<AirDropFile>,
    #[serde(rename = "FileIcon")]
    pub file_icon: Vec<u8>,
    #[serde(rename = "SenderModelName")]
    pub sender_model_name: Option<String>,
}

/// File metadata in an AirDrop Ask request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirDropFile {
    #[serde(rename = "FileName")]
    pub file_name: String,
    #[serde(rename = "FileType")]
    pub file_type: String,
    #[serde(rename = "FileSubtype")]
    pub file_subtype: Option<String>,
    #[serde(rename = "FileSize")]
    pub file_size: u64,
    #[serde(rename = "FileHash")]
    pub file_hash: Option<Vec<u8>>,
}

/// AirDrop Ask response (receiver accepts or rejects)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirDropAskResponse {
    #[serde(rename = "ReceiverComputerName")]
    pub receiver_computer_name: String,
    #[serde(rename = "ReceiverModelName")]
    pub receiver_model_name: String,
}

/// AirDrop Upload response (after file transfer)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirDropUploadResponse {
    #[serde(rename = "ReceiverComputerName")]
    pub receiver_computer_name: String,
    #[serde(rename = "ReceiverModelName")]
    pub receiver_model_name: String,
}

// ============================================================================
// Events
// ============================================================================

/// Events emitted by the AirDrop service
#[derive(Debug, Clone)]
pub enum AirDropEvent {
    /// A nearby Apple device wants to send a file
    IncomingTransferRequest {
        transfer_id: String,
        sender_name: String,
        file_name: String,
        file_size: u64,
    },
    /// File transfer completed
    TransferCompleted {
        transfer_id: String,
        file_path: PathBuf,
    },
    /// File transfer failed
    TransferFailed {
        transfer_id: String,
        error: String,
    },
    /// An AirDrop-capable device was discovered
    DeviceFound {
        device_id: String,
        device_name: String,
        ip: IpAddr,
        port: u16,
    },
    /// An AirDrop device was lost
    DeviceLost {
        device_id: String,
    },
}

// ============================================================================
// AirDrop TLS Certificate
// ============================================================================

/// Generate a self-signed TLS certificate for the AirDrop HTTPS server.
/// In "Everyone" mode, Apple devices accept any valid TLS certificate.
/// In "Contacts Only" mode, Apple-signed certificates are required (not supported here).
fn generate_airdrop_certificate(
    device_name: &str,
) -> Result<(CertificateDer<'static>, PrivatePkcs8KeyDer<'static>)> {
    let key_pair = KeyPair::generate().map_err(|e| {
        ConnectedError::Connection(format!("Failed to generate TLS key pair: {}", e))
    })?;

    let mut params = CertificateParams::new(vec![device_name.to_string()]).map_err(|e| {
        ConnectedError::Connection(format!("Failed to create certificate params: {}", e))
    })?;

    params.distinguished_name.push(
        rcgen::DnType::CommonName,
        rcgen::DnValue::Utf8String(device_name.to_string()),
    );

    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| ConnectedError::Connection(format!("Failed to sign certificate: {}", e)))?;

    let cert_der = CertificateDer::from(cert.der().to_vec());
    let key_der = PrivatePkcs8KeyDer::from(key_pair.serialize_der());

    Ok((cert_der, key_der))
}

// ============================================================================
// AirDrop Server
// ============================================================================

/// The AirDrop protocol server that makes Connected discoverable by Apple devices.
///
/// This implements the AirDrop application-layer protocol:
/// - mDNS service announcement (`_airdrop._tcp.local.`)
/// - HTTPS server with `/Discover`, `/Ask`, `/Upload` endpoints
/// - Plist-based message format
/// - CPIO archive for file transfer
pub struct AirDropServer {
    device: Device,
    service_info: AirDropServiceInfo,
    mdns_daemon: ServiceDaemon,
    event_tx: mpsc::UnboundedSender<AirDropEvent>,
    download_dir: Arc<RwLock<PathBuf>>,
}

impl AirDropServer {
    /// Create a new AirDrop server for the given device.
    pub fn new(
        device: Device,
        event_tx: mpsc::UnboundedSender<AirDropEvent>,
        download_dir: PathBuf,
    ) -> Result<Self> {
        let mdns_daemon = ServiceDaemon::new().map_err(|e| {
            ConnectedError::Connection(format!("Failed to create mDNS daemon: {}", e))
        })?;

        let service_info = AirDropServiceInfo {
            device_id: device.id.clone(),
            device_name: device.name.clone(),
            port: 0, // Will be set when server starts
            device_type: device.device_type,
            discoverable_everyone: true,
        };

        Ok(Self {
            device,
            service_info,
            mdns_daemon,
            event_tx,
            download_dir: Arc::new(RwLock::new(download_dir)),
        })
    }

    /// Start the AirDrop server on the given port (0 for auto-select).
    /// Returns the actual port the server is listening on.
    pub async fn start(&mut self, port: u16) -> Result<u16> {
        // Generate TLS certificate
        let (cert_der, key_der) = generate_airdrop_certificate(&self.device.name)?;

        // Build TLS config
        let server_config =
            rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(vec![cert_der], key_der.into())?;

        let tls_acceptor = TlsAcceptor::from(Arc::new(server_config));

        // Bind TCP listener
        let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port);
        let listener = TcpListener::bind(bind_addr).await?;
        let actual_port = listener.local_addr()?.port();

        info!("AirDrop HTTPS server listening on port {}", actual_port);

        // Update service info with actual port
        self.service_info.port = actual_port;

        // Register mDNS service
        self.announce_service(actual_port)?;

        // Spawn accept loop
        let event_tx = self.event_tx.clone();
        let download_dir = self.download_dir.clone();
        let device_name = self.device.name.clone();
        let device_type = self.device.device_type;
        let device_id = self.device.id.clone();

        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, addr)) => {
                        debug!("AirDrop connection from {}", addr);
                        let tls_acceptor = tls_acceptor.clone();
                        let event_tx = event_tx.clone();
                        let download_dir = download_dir.clone();
                        let device_name = device_name.clone();
                        let device_type_str = format!("{:?}", device_type);
                        let device_id = device_id.clone();

                        tokio::spawn(async move {
                            match tls_acceptor.accept(stream).await {
                                Ok(tls_stream) => {
                                    let io = TokioIo::new(tls_stream);
                                    let service = service_fn(|req| {
                                        handle_airdrop_request(
                                            req,
                                            event_tx.clone(),
                                            download_dir.clone(),
                                            device_name.clone(),
                                            device_type_str.clone(),
                                            device_id.clone(),
                                        )
                                    });
                                    if let Err(e) = http1::Builder::new()
                                        .serve_connection(io, service)
                                        .await
                                    {
                                        debug!("AirDrop HTTP connection error: {}", e);
                                    }
                                }
                                Err(e) => {
                                    debug!("AirDrop TLS handshake failed: {}", e);
                                }
                            }
                        });
                    }
                    Err(e) => {
                        error!("AirDrop accept error: {}", e);
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        });

        Ok(actual_port)
    }

    /// Announce the AirDrop service via mDNS.
    fn announce_service(&self, port: u16) -> Result<()> {
        let service_id = format!("{}._airdrop._tcp", self.device_id_short());

        // Determine device subtype for TXT record
        let device_sub_type = match self.device.device_type {
            DeviceType::MacOS => "Computer",
            DeviceType::IOS => "Phone",
            DeviceType::Windows => "Computer",
            DeviceType::Linux => "Computer",
            DeviceType::Android => "Phone",
            _ => "Computer",
        };

        // Build TXT record properties
        let mut properties = HashMap::new();
        properties.insert("flags".to_string(), "0x00".to_string()); // Everyone mode
        properties.insert("id".to_string(), self.device.id.clone());
        properties.insert("models".to_string(), format!("{}:{}", device_sub_type, self.device.name));

        let local_ip = self.get_local_ip().unwrap_or(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)));

        let mut service_info = ServiceInfo::new(
            AIRDROP_SERVICE_TYPE,
            &service_id,
            &format!("{}.local.", service_id),
            local_ip,
            port,
            properties,
        )
        .map_err(|e| {
            ConnectedError::Connection(format!("Failed to create mDNS service info: {}", e))
        })?;

        // Enable sharing
        service_info = service_info.enable_addr_auto();

        self.mdns_daemon
            .register(service_info)
            .map_err(|e| {
                ConnectedError::Connection(format!("Failed to register AirDrop mDNS service: {}", e))
            })?;

        info!(
            "AirDrop mDNS service registered: {} on port {}",
            service_id, port
        );
        Ok(())
    }

    /// Get a short device ID for mDNS service naming.
    fn device_id_short(&self) -> &str {
        if self.device.id.len() > 8 {
            &self.device.id[..8]
        } else {
            &self.device.id
        }
    }

    /// Get the first non-loopback IPv4 address.
    fn get_local_ip(&self) -> Option<IpAddr> {
        if let Ok(addrs) = if_addrs::get_if_addrs() {
            for addr in addrs {
                if !addr.ip().is_loopback() && addr.ip().is_ipv4() {
                    return Some(addr.ip());
                }
            }
        }
        None
    }

    /// Stop the AirDrop server and unregister mDNS service.
    pub fn stop(&self) {
        let service_id = format!("{}._airdrop._tcp", self.device_id_short());
        if let Err(e) = self.mdns_daemon.unregister(&format!("{}.{}", service_id, AIRDROP_SERVICE_TYPE)) {
            debug!("Failed to unregister AirDrop mDNS service: {}", e);
        }
        info!("AirDrop server stopped");
    }
}

// ============================================================================
// HTTP Request Handler
// ============================================================================

/// Handle incoming AirDrop HTTP requests.
async fn handle_airdrop_request(
    req: Request<Incoming>,
    event_tx: mpsc::UnboundedSender<AirDropEvent>,
    download_dir: Arc<RwLock<PathBuf>>,
    device_name: String,
    device_type: String,
    device_id: String,
) -> std::result::Result<Response<Full<Bytes>>, hyper::Error> {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let path = uri.path().to_string();

    debug!("AirDrop {} {}", method, path);

    match path.as_str() {
        "/Discover" => handle_discover(req, &device_name, &device_type).await,
        "/Ask" => {
            handle_ask(
                req,
                &event_tx,
                &device_name,
                &device_type,
                &device_id,
            )
            .await
        }
        "/Upload" => {
            handle_upload(req, &event_tx, &download_dir).await
        }
        _ => {
            let mut response = Response::new(Full::new(Bytes::from("Not Found")));
            *response.status_mut() = StatusCode::NOT_FOUND;
            Ok(response)
        }
    }
}

/// Handle the `/Discover` endpoint.
/// The sender sends its record data; the receiver responds with device info.
async fn handle_discover(
    req: Request<Incoming>,
    device_name: &str,
    device_type: &str,
) -> std::result::Result<Response<Full<Bytes>>, hyper::Error> {
    // Read the request body (sender's record data as plist)
    let _body = req.into_body().collect().await?.to_bytes();

    // Build the Discover response
    let response = AirDropDiscoverResponse {
        receiver_computer_name: device_name.to_string(),
        receiver_media_capabilities: "{}".to_string(),
        receiver_model_name: device_type.to_string(),
    };

    let plist_data = serialize_plist_binary(&response);

    let mut http_response = Response::new(Full::new(Bytes::from(plist_data)));
    http_response
        .headers_mut()
        .insert("Content-Type", "application/x-apple-binary-plist".parse().unwrap());
    Ok(http_response)
}

/// Handle the `/Ask` endpoint.
/// The sender asks permission to send a file; the receiver responds with accept/reject.
async fn handle_ask(
    req: Request<Incoming>,
    event_tx: &mpsc::UnboundedSender<AirDropEvent>,
    device_name: &str,
    device_type: &str,
    device_id: &str,
) -> std::result::Result<Response<Full<Bytes>>, hyper::Error> {
    // Read the request body (Ask plist)
    let body = req.into_body().collect().await?.to_bytes();

    // Parse the Ask request
    let ask_request: AirDropAskRequest = match plist::from_bytes(&body) {
        Ok(req) => req,
        Err(e) => {
            warn!("Failed to parse AirDrop Ask request: {}", e);
            let mut response = Response::new(Full::new(Bytes::from("Bad Request")));
            *response.status_mut() = StatusCode::BAD_REQUEST;
            return Ok(response);
        }
    };

    info!(
        "AirDrop Ask from '{}': {} files",
        ask_request.sender_computer_name,
        ask_request.files.len()
    );

    // For each file in the request, emit an incoming transfer event
    for file in &ask_request.files {
        let transfer_id = format!(
            "airdrop-{}-{}",
            device_id,
            uuid::Uuid::new_v4()
        );

        let _ = event_tx.send(AirDropEvent::IncomingTransferRequest {
            transfer_id,
            sender_name: ask_request.sender_computer_name.clone(),
            file_name: file.file_name.clone(),
            file_size: file.file_size,
        });
    }

    // Accept the transfer (auto-accept for now; could add user confirmation later)
    let response = AirDropAskResponse {
        receiver_computer_name: device_name.to_string(),
        receiver_model_name: device_type.to_string(),
    };

    let plist_data = serialize_plist_binary(&response);

    let mut http_response = Response::new(Full::new(Bytes::from(plist_data)));
    http_response
        .headers_mut()
        .insert("Content-Type", "application/x-apple-binary-plist".parse().unwrap());
    Ok(http_response)
}

/// Handle the `/Upload` endpoint.
/// The sender uploads the file as a CPIO archive.
async fn handle_upload(
    req: Request<Incoming>,
    event_tx: &mpsc::UnboundedSender<AirDropEvent>,
    download_dir: &Arc<RwLock<PathBuf>>,
) -> std::result::Result<Response<Full<Bytes>>, hyper::Error> {
    let content_type = req
        .headers()
        .get("Content-Type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    info!("AirDrop Upload: Content-Type={}", content_type);

    // Read the uploaded data (CPIO archive)
    let body = req.into_body().collect().await?.to_bytes();
    let data = body.to_vec();

    // Extract files from CPIO archive
    let transfer_id = format!("airdrop-upload-{}", uuid::Uuid::new_v4());
    match extract_cpio_archive(&data, download_dir) {
        Ok(files) => {
            for file_path in files {
                info!("AirDrop file received: {:?}", file_path);
                let _ = event_tx.send(AirDropEvent::TransferCompleted {
                    transfer_id: transfer_id.clone(),
                    file_path,
                });
            }
        }
        Err(e) => {
            error!("Failed to extract AirDrop CPIO archive: {}", e);
            let _ = event_tx.send(AirDropEvent::TransferFailed {
                transfer_id: transfer_id.clone(),
                error: e.to_string(),
            });
        }
    }

    // Build the Upload response
    let response = AirDropUploadResponse {
        receiver_computer_name: "Connected".to_string(),
        receiver_model_name: "Connected".to_string(),
    };

    let plist_data = serialize_plist_binary(&response);

    let mut http_response = Response::new(Full::new(Bytes::from(plist_data)));
    http_response
        .headers_mut()
        .insert("Content-Type", "application/x-apple-binary-plist".parse().unwrap());
    Ok(http_response)
}

// ============================================================================
// CPIO Archive Handling
// ============================================================================

/// Extract files from a gzip-compressed CPIO archive into the download directory.
fn extract_cpio_archive(
    data: &[u8],
    download_dir: &Arc<RwLock<PathBuf>>,
) -> Result<Vec<PathBuf>> {
    use flate2::read::GzDecoder;
    use std::io::Read;

    let mut decoder = GzDecoder::new(data);
    let mut decoded_data = Vec::new();
    decoder
        .read_to_end(&mut decoded_data)
        .map_err(|e| ConnectedError::Connection(format!("Failed to decompress gzip: {}", e)))?;

    // Parse CPIO archive (newc format)
    let mut files = Vec::new();
    let mut offset = 0;
    let dir = download_dir.read().clone();

    while offset + 110 <= decoded_data.len() {
        // New CPIO header is 110 bytes
        let header = &decoded_data[offset..offset + 110];

        // Check magic number "070701" or "070702"
        if &header[0..6] != b"070701" && &header[0..6] != b"070702" {
            break;
        }

        // Parse file size (hex, 8 bytes at offset 54)
        let file_size_hex = std::str::from_utf8(&header[54..62])
            .map_err(|_| ConnectedError::Connection("Invalid CPIO header".into()))?;
        let file_size = u64::from_str_radix(file_size_hex, 16)
            .map_err(|_| ConnectedError::Connection("Invalid file size in CPIO".into()))?;

        // Parse filename length (hex, 8 bytes at offset 94)
        let namesize_hex = std::str::from_utf8(&header[94..102])
            .map_err(|_| ConnectedError::Connection("Invalid CPIO header".into()))?;
        let namesize = u64::from_str_radix(namesize_hex, 16)
            .map_err(|_| ConnectedError::Connection("Invalid name size in CPIO".into()))?;

        // Read filename
        let name_start = offset + 110;
        let name_end = name_start + namesize as usize;
        if name_end > decoded_data.len() {
            break;
        }
        let name_bytes = &decoded_data[name_start..name_end];
        // Remove null terminator
        let name = String::from_utf8_lossy(
            &name_bytes[..name_bytes.iter().position(|&b| b == 0).unwrap_or(name_bytes.len())],
        )
        .to_string();

        // Skip to file data (header + namesize, padded to 4-byte boundary)
        let data_start = name_start + namesize as usize;
        let data_start = (data_start + 3) & !3; // Align to 4 bytes

        // Special entries: "." and "TRAILER!!!" mark end of archive
        if name == "." || name == "TRAILER!!!" {
            break;
        }

        // Skip directories (they have file_size 0 or name ending with /)
        if name.ends_with('/') || file_size == 0 {
            offset = data_start + file_size as usize;
            offset = (offset + 3) & !3; // Align to 4 bytes
            continue;
        }

        // Extract file
        let file_data_end = data_start + file_size as usize;
        if file_data_end <= decoded_data.len() {
            let file_name = PathBuf::from(&name)
                .file_name()
                .map(|n| n.to_owned())
                .unwrap_or_else(|| "unknown".into());

            let file_path = dir.join(&file_name);

            std::fs::write(&file_path, &decoded_data[data_start..file_data_end]).map_err(|e| {
                ConnectedError::Connection(format!("Failed to write file {}: {}", name, e))
            })?;

            info!("Extracted AirDrop file: {:?}", file_path);
            files.push(file_path);
        }

        offset = data_start + file_size as usize;
        offset = (offset + 3) & !3; // Align to 4 bytes
    }

    if files.is_empty() && !decoded_data.is_empty() {
        // Fallback: save raw data as a single file
        let fallback_name = format!("airdrop-{}", uuid::Uuid::new_v4());
        let file_path = dir.join(&fallback_name);
        std::fs::write(&file_path, &decoded_data).map_err(|e| {
            ConnectedError::Connection(format!("Failed to save raw AirDrop data: {}", e))
        })?;
        files.push(file_path);
    }

    Ok(files)
}

// ============================================================================
// AirDrop Client (for sending to Apple devices)
// ============================================================================

/// Client for sending files to Apple devices via AirDrop.
#[allow(dead_code)]
pub struct AirDropClient {
    device_name: String,
}

impl AirDropClient {
    pub fn new(device_name: String) -> Self {
        Self { device_name }
    }

    /// Send a file to an AirDrop receiver.
    /// This is a placeholder - the full implementation requires proper
    /// hyper client setup with TLS and the 3-step AirDrop protocol flow
    /// (Discover -> Ask -> Upload).
    pub async fn send_file(
        &self,
        _target_addr: SocketAddr,
        _file_path: &PathBuf,
        _file_name: &str,
    ) -> Result<()> {
        // TODO: Implement full AirDrop send protocol:
        // 1. TLS connect to target
        // 2. POST /Discover with plist
        // 3. POST /Ask with file metadata plist
        // 4. POST /Upload with gzipped CPIO archive
        warn!("AirDrop client send_file not yet fully implemented");
        Ok(())
    }
}

/// TLS certificate verifier that accepts any certificate (for "Everyone" mode).
#[derive(Debug)]
#[allow(dead_code)]
struct NoVerifier;

impl rustls::client::danger::ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dbsig: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dbsig: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
        ]
    }
}

// ============================================================================
// CPIO Archive Creation
// ============================================================================

/// Create a CPIO archive (newc format) containing a single file.
#[allow(dead_code)]
fn create_cpio_archive(file_name: &str, file_data: &[u8]) -> Result<Vec<u8>> {
    let mut output = Vec::new();

    // CPIO newc header format:
    // magic(6) + dev(8) + ino(8) + mode(8) + uid(8) + gid(8) + nlink(8)
    // + rdev(8) + mtime(8) + namesize(8) + checksize(8) = 110 bytes
    // + filename + padding to 4 bytes + file data + padding to 4 bytes

    let name = file_name.as_bytes();
    let namesize = name.len() + 1; // Include null terminator
    let filesize = file_data.len();

    // Build header
    let header = format!(
        "070701{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}",
        0,      // dev
        1,      // ino
        0o100644, // mode (regular file, 644)
        0,      // uid
        0,      // gid
        1,      // nlink
        0,      // rdev
        0,      // mtime
        namesize,
        filesize,
    );

    output.extend_from_slice(header.as_bytes());
    output.extend_from_slice(name);
    output.push(0); // null terminator

    // Pad to 4-byte boundary
    while output.len() % 4 != 0 {
        output.push(0);
    }

    // File data
    output.extend_from_slice(file_data);

    // Pad to 4-byte boundary
    while output.len() % 4 != 0 {
        output.push(0);
    }

    // Write TRAILER!!! entry
    let trailer = format!(
        "070701{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}",
        0,      // dev
        0,      // ino
        0,      // mode
        0,      // uid
        0,      // gid
        0,      // nlink
        0,      // rdev
        0,      // mtime
        11,     // namesize ("TRAILER!!!" + null)
        0,      // filesize
    );
    output.extend_from_slice(trailer.as_bytes());
    output.extend_from_slice(b"TRAILER!!!");
    output.push(0); // null terminator

    // Pad to 4-byte boundary
    while output.len() % 4 != 0 {
        output.push(0);
    }

    Ok(output)
}

// ============================================================================
// UTI Type Mapping
// ============================================================================

/// Map a file path to an Apple UTI type based on its extension.
#[allow(dead_code)]
fn get_uti_type(path: &std::path::Path) -> String {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "public.jpeg".to_string(),
        "png" => "public.png".to_string(),
        "gif" => "com.compuserve.gif".to_string(),
        "tiff" | "tif" => "public.tiff".to_string(),
        "bmp" => "com.microsoft.bmp".to_string(),
        "heic" => "public.heic".to_string(),
        "heif" => "public.heif".to_string(),
        "mp4" | "m4v" => "public.mpeg-4".to_string(),
        "mov" => "com.apple.quicktime-movie".to_string(),
        "mp3" => "public.mp3".to_string(),
        "m4a" => "public.mpeg-4-audio".to_string(),
        "wav" => "com.microsoft.waveform-audio".to_string(),
        "pdf" => "com.adobe.pdf".to_string(),
        "txt" => "public.plain-text".to_string(),
        "zip" => "public.zip-archive".to_string(),
        "vcf" => "public.vcard".to_string(),
        _ => "public.data".to_string(),
    }
}

// ============================================================================
// Hyper HTTP Executor
// ============================================================================

#[derive(Clone)]
#[allow(dead_code)]
struct TokioHttpExecutor;

impl hyper::rt::Executor<std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>>
    for TokioHttpExecutor
{
    fn execute(
        &self,
        future: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>,
    ) {
        tokio::spawn(future);
    }
}
