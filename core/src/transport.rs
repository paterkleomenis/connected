use crate::error::{ConnectedError, Result};
use crate::security::KeyStore;
use parking_lot::RwLock;
use quinn::{
    ClientConfig, Connection, Endpoint, RecvStream, SendStream, ServerConfig, TransportConfig,
    VarInt,
};
use rustls::client::danger::HandshakeSignatureValid;
use rustls::pki_types::CertificateDer;
use rustls::DistinguishedName;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

const ALPN_PROTOCOL: &[u8] = b"connected/1";
const PING_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_MESSAGE_SIZE: usize = 64 * 1024; // 64KB for control messages

// LAN-optimized transport parameters
const INITIAL_RTT_MS: u64 = 10;
const MAX_IDLE_TIMEOUT_SECS: u64 = 60;
const KEEP_ALIVE_INTERVAL_SECS: u64 = 15;
const MAX_CONCURRENT_BIDI_STREAMS: u32 = 128;
const MAX_CONCURRENT_UNI_STREAMS: u32 = 128;
const STREAM_RECEIVE_WINDOW: u32 = 16 * 1024 * 1024; // 16MB per stream
const CONNECTION_RECEIVE_WINDOW: u32 = 64 * 1024 * 1024; // 64MB per connection
const SEND_WINDOW: u64 = 16 * 1024 * 1024; // 16MB send window

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Message {
    Ping {
        from_id: String,
        timestamp: u64,
    },
    Pong {
        from_id: String,
        timestamp: u64,
    },
    Handshake {
        device_id: String,
        device_name: String,
    },
    HandshakeAck {
        device_id: String,
        device_name: String,
    },
    Clipboard {
        text: String,
    },
    FileTransfer,
}

struct ConnectionCache {
    connections: HashMap<SocketAddr, CachedConnection>,
}

struct CachedConnection {
    connection: Connection,
    last_used: std::time::Instant,
}

impl ConnectionCache {
    fn new() -> Self {
        Self {
            connections: HashMap::new(),
        }
    }

    fn get(&mut self, addr: &SocketAddr) -> Option<Connection> {
        if let Some(cached) = self.connections.get_mut(addr) {
            if cached.connection.close_reason().is_none() {
                cached.last_used = std::time::Instant::now();
                return Some(cached.connection.clone());
            } else {
                self.connections.remove(addr);
            }
        }
        None
    }

    fn insert(&mut self, addr: SocketAddr, connection: Connection) {
        let cutoff = std::time::Instant::now() - Duration::from_secs(300);
        self.connections
            .retain(|_, v| v.last_used > cutoff && v.connection.close_reason().is_none());

        self.connections.insert(
            addr,
            CachedConnection {
                connection,
                last_used: std::time::Instant::now(),
            },
        );
    }

    fn remove(&mut self, addr: &SocketAddr) {
        self.connections.remove(addr);
    }
}

pub struct QuicTransport {
    endpoint: Endpoint,
    local_id: String,
    client_config: ClientConfig,
    connection_cache: Arc<RwLock<ConnectionCache>>,
    _key_store: Arc<RwLock<KeyStore>>,
}

impl QuicTransport {
    pub async fn new(
        bind_addr: SocketAddr,
        local_id: String,
        key_store: Arc<RwLock<KeyStore>>,
    ) -> Result<Self> {
        let (server_config, _) = Self::create_server_config(&key_store)?;
        let client_config = Self::create_client_config(&key_store)?;

        let mut endpoint = Endpoint::server(server_config, bind_addr)?;
        endpoint.set_default_client_config(client_config.clone());

        info!("QUIC transport listening on {}", bind_addr);

        Ok(Self {
            endpoint,
            local_id,
            client_config,
            connection_cache: Arc::new(RwLock::new(ConnectionCache::new())),
            _key_store: key_store,
        })
    }

    fn create_transport_config() -> TransportConfig {
        let mut transport = TransportConfig::default();
        transport.initial_rtt(Duration::from_millis(INITIAL_RTT_MS));
        transport.max_idle_timeout(Some(
            Duration::from_secs(MAX_IDLE_TIMEOUT_SECS)
                .try_into()
                .unwrap(),
        ));
        transport.keep_alive_interval(Some(Duration::from_secs(KEEP_ALIVE_INTERVAL_SECS)));
        transport.max_concurrent_bidi_streams(VarInt::from_u32(MAX_CONCURRENT_BIDI_STREAMS));
        transport.max_concurrent_uni_streams(VarInt::from_u32(MAX_CONCURRENT_UNI_STREAMS));
        transport.stream_receive_window(VarInt::from_u32(STREAM_RECEIVE_WINDOW));
        transport.receive_window(VarInt::from_u32(CONNECTION_RECEIVE_WINDOW));
        transport.send_window(SEND_WINDOW);
        transport.datagram_receive_buffer_size(None);
        transport.ack_frequency_config(None);
        transport.initial_mtu(1400);
        transport.min_mtu(1200);
        transport.mtu_discovery_config(Some(Default::default()));

        transport
    }

    pub fn create_server_config_detached(
        key_store: &Arc<RwLock<KeyStore>>,
    ) -> Result<ServerConfig> {
        let (server_config, _) = Self::create_server_config(key_store)?;
        Ok(server_config)
    }

    fn create_server_config(
        key_store: &Arc<RwLock<KeyStore>>,
    ) -> Result<(ServerConfig, CertificateDer<'static>)> {
        let ks = key_store.read();
        let cert = ks.get_cert();
        let key = ks.get_key();

        let client_verifier = Arc::new(ClientVerifier {
            key_store: key_store.clone(),
        });

        let mut server_crypto = rustls::ServerConfig::builder()
            .with_client_cert_verifier(client_verifier)
            .with_single_cert(vec![cert.clone()], key.into())?;

        server_crypto.alpn_protocols = vec![ALPN_PROTOCOL.to_vec()];
        server_crypto.max_early_data_size = u32::MAX;

        let mut server_config = ServerConfig::with_crypto(Arc::new(
            quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)
                .map_err(|e| ConnectedError::Tls(rustls::Error::General(e.to_string())))?,
        ));

        server_config.transport_config(Arc::new(Self::create_transport_config()));

        Ok((server_config, cert))
    }

    fn create_client_config(key_store: &Arc<RwLock<KeyStore>>) -> Result<ClientConfig> {
        let ks = key_store.read();
        let cert = ks.get_cert();
        let key = ks.get_key();

        let mut client_crypto = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(PeerVerifier {
                key_store: key_store.clone(),
            }))
            .with_client_auth_cert(vec![cert], key.into())?;

        client_crypto.alpn_protocols = vec![ALPN_PROTOCOL.to_vec()];
        client_crypto.enable_early_data = true;

        let mut client_config = ClientConfig::new(Arc::new(
            quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto)
                .map_err(|e| ConnectedError::Tls(rustls::Error::General(e.to_string())))?,
        ));

        client_config.transport_config(Arc::new(Self::create_transport_config()));

        Ok(client_config)
    }

    pub async fn connect(&self, addr: SocketAddr) -> Result<Connection> {
        {
            let mut cache = self.connection_cache.write();
            if let Some(conn) = cache.get(&addr) {
                debug!("Reusing cached connection to {}", addr);
                return Ok(conn);
            }
        }

        debug!("Establishing new connection to {}", addr);

        let connecting =
            self.endpoint
                .connect_with(self.client_config.clone(), addr, "connected.local")?;

        let connection = timeout(CONNECT_TIMEOUT, connecting)
            .await
            .map_err(|_| ConnectedError::Timeout("Connection timeout".to_string()))?
            .map_err(|e| ConnectedError::Connection(e.to_string()))?;

        info!("Connected to {} (RTT: {:?})", addr, connection.rtt());

        {
            let mut cache = self.connection_cache.write();
            cache.insert(addr, connection.clone());
        }

        Ok(connection)
    }

    pub fn invalidate_connection(&self, addr: &SocketAddr) {
        let mut cache = self.connection_cache.write();
        cache.remove(addr);
        debug!("Invalidated cached connection to {}", addr);
    }

    pub async fn send_ping(&self, target_addr: SocketAddr) -> Result<Duration> {
        let start = std::time::Instant::now();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let connection = self.connect(target_addr).await?;

        let (mut send, mut recv) = connection.open_bi().await.map_err(|e| {
            self.invalidate_connection(&target_addr);
            ConnectedError::Connection(e.to_string())
        })?;

        // Write Stream Type
        send.write_all(&[Self::STREAM_TYPE_CONTROL]).await?;

        let ping = Message::Ping {
            from_id: self.local_id.clone(),
            timestamp,
        };

        let ping_data = serde_json::to_vec(&ping)?;
        let len_bytes = (ping_data.len() as u32).to_be_bytes();
        send.write_all(&len_bytes).await?;
        send.write_all(&ping_data).await?;
        send.finish()?;

        let response = timeout(PING_TIMEOUT, async {
            let mut len_buf = [0u8; 4];
            recv.read_exact(&mut len_buf).await?;
            let msg_len = u32::from_be_bytes(len_buf) as usize;
            if msg_len > MAX_MESSAGE_SIZE {
                return Err(quinn::ReadExactError::FinishedEarly(0));
            }
            let mut data = vec![0u8; msg_len];
            recv.read_exact(&mut data).await?;
            Ok(data)
        })
        .await;

        match response {
            Ok(Ok(data)) => {
                let message: Message = serde_json::from_slice(&data)?;
                match message {
                    Message::Pong {
                        from_id,
                        timestamp: pong_ts,
                    } => {
                        if pong_ts == timestamp {
                            let rtt = start.elapsed();
                            info!(
                                "Ping successful to {} (RTT: {:?}, QUIC RTT: {:?})",
                                from_id,
                                rtt,
                                connection.rtt()
                            );
                            Ok(rtt)
                        } else {
                            Err(ConnectedError::PingFailed("Timestamp mismatch".to_string()))
                        }
                    }
                    _ => Err(ConnectedError::PingFailed(
                        "Unexpected response".to_string(),
                    )),
                }
            }
            Ok(Err(e)) => {
                self.invalidate_connection(&target_addr);
                Err(ConnectedError::PingFailed(e.to_string()))
            }
            Err(_) => {
                self.invalidate_connection(&target_addr);
                Err(ConnectedError::Timeout("Ping timeout".to_string()))
            }
        }
    }

    pub const STREAM_TYPE_CONTROL: u8 = 1;

    pub const STREAM_TYPE_FILE: u8 = 2;

    pub async fn start_server(
        &self,

        message_tx: mpsc::UnboundedSender<(SocketAddr, String, Message)>,

        file_stream_tx: mpsc::UnboundedSender<(String, SendStream, RecvStream)>,
    ) -> Result<()> {
        let endpoint = self.endpoint.clone();

        let local_id = self.local_id.clone();

        tokio::spawn(async move {
            info!("QUIC server started, waiting for connections");

            while let Some(incoming) = endpoint.accept().await {
                let tx = message_tx.clone();

                let f_tx = file_stream_tx.clone();

                let id = local_id.clone();

                tokio::spawn(async move {
                    match incoming.accept() {
                        Ok(connecting) => match connecting.await {
                            Ok(connection) => {
                                let remote_addr = connection.remote_address();

                                debug!(
                                    "Accepted connection from {} (RTT: {:?})",
                                    remote_addr,
                                    connection.rtt()
                                );

                                if let Err(e) =
                                    Self::handle_connection(connection, remote_addr, tx, f_tx, id)
                                        .await
                                {
                                    warn!("Connection handler error: {}", e);
                                }
                            }

                            Err(e) => {
                                warn!("Failed to complete connection: {}", e);
                            }
                        },

                        Err(e) => {
                            warn!("Failed to accept connection: {}", e);
                        }
                    }
                });
            }
        });

        Ok(())
    }

    async fn handle_connection(
        connection: Connection,

        remote_addr: SocketAddr,

        message_tx: mpsc::UnboundedSender<(SocketAddr, String, Message)>,

        file_stream_tx: mpsc::UnboundedSender<(String, SendStream, RecvStream)>,

        local_id: String,
    ) -> Result<()> {
        let fingerprint =
            Self::get_peer_fingerprint(&connection).unwrap_or_else(|| "unknown".to_string());

        loop {
            match connection.accept_bi().await {
                Ok((mut send, mut recv)) => {
                    // Read Stream Type

                    let mut type_buf = [0u8; 1];

                    if recv.read_exact(&mut type_buf).await.is_err() {
                        continue;
                    }

                    match type_buf[0] {
                        Self::STREAM_TYPE_CONTROL => {
                            let mut len_buf = [0u8; 4];

                            if recv.read_exact(&mut len_buf).await.is_err() {
                                continue;
                            }

                            let msg_len = u32::from_be_bytes(len_buf) as usize;

                            if msg_len == 0 || msg_len > MAX_MESSAGE_SIZE {
                                continue;
                            }

                            let mut data = vec![0u8; msg_len];

                            if recv.read_exact(&mut data).await.is_err() {
                                continue;
                            }

                            let message: Message = match serde_json::from_slice(&data) {
                                Ok(m) => m,

                                Err(e) => {
                                    debug!("Failed to parse message: {}", e);

                                    continue;
                                }
                            };

                            debug!(
                                "Received message from {} ({}): {:?}",
                                remote_addr, fingerprint, message
                            );

                            match &message {
                                Message::Ping { timestamp, .. } => {
                                    let pong = Message::Pong {
                                        from_id: local_id.clone(),

                                        timestamp: *timestamp,
                                    };

                                    let pong_data = serde_json::to_vec(&pong)?;

                                    let len_bytes = (pong_data.len() as u32).to_be_bytes();

                                    // Send Stream Type + Length + Data

                                    send.write_all(&[Self::STREAM_TYPE_CONTROL]).await?;

                                    send.write_all(&len_bytes).await?;

                                    send.write_all(&pong_data).await?;

                                    send.finish()?;
                                }

                                _ => {
                                    let _ = message_tx.send((
                                        remote_addr,
                                        fingerprint.clone(),
                                        message,
                                    ));
                                }
                            }
                        }

                        Self::STREAM_TYPE_FILE => {
                            // Hand off the stream to the file handler

                            info!("Received File Stream from {}", fingerprint);

                            let _ = file_stream_tx.send((fingerprint.clone(), send, recv));
                        }

                        _ => {
                            warn!("Unknown stream type: {}", type_buf[0]);
                        }
                    }
                }

                Err(quinn::ConnectionError::ApplicationClosed(reason)) => {
                    debug!(
                        "Connection closed by peer: {} (reason: {:?})",
                        remote_addr, reason
                    );

                    break;
                }

                Err(quinn::ConnectionError::LocallyClosed) => {
                    debug!("Connection closed locally: {}", remote_addr);

                    break;
                }

                Err(quinn::ConnectionError::TimedOut) => {
                    debug!("Connection timed out: {}", remote_addr);

                    break;
                }

                Err(e) => {
                    error!("Stream error: {}", e);

                    break;
                }
            }
        }

        Ok(())
    }

    pub async fn open_stream(
        &self,
        addr: SocketAddr,
        stream_type: u8,
    ) -> Result<(SendStream, RecvStream)> {
        let connection = self.connect(addr).await?;

        let (mut send, recv) = connection.open_bi().await?;

        send.write_all(&[stream_type]).await?;

        Ok((send, recv))
    }

    fn get_peer_fingerprint(connection: &Connection) -> Option<String> {
        let identity = connection.peer_identity()?;
        let certs = identity.downcast_ref::<Vec<rustls::pki_types::CertificateDer>>()?;
        if let Some(cert) = certs.first() {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(cert.as_ref());
            Some(format!("{:x}", hasher.finalize()))
        } else {
            None
        }
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.endpoint.local_addr().map_err(ConnectedError::Io)
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    pub fn stats(&self) -> TransportStats {
        let cache = self.connection_cache.read();
        let active_connections = cache
            .connections
            .iter()
            .filter(|(_, c)| c.connection.close_reason().is_none())
            .count();

        TransportStats {
            active_connections,
            cached_connections: cache.connections.len(),
        }
    }

    pub async fn shutdown(&self) {
        {
            let cache = self.connection_cache.read();
            for (addr, cached) in cache.connections.iter() {
                debug!("Closing connection to {}", addr);
                cached.connection.close(VarInt::from_u32(0), b"shutdown");
            }
        }

        self.endpoint.close(VarInt::from_u32(0), b"shutdown");
        self.endpoint.wait_idle().await;
        info!("QUIC transport shut down");
    }
}

#[derive(Debug, Clone)]
pub struct TransportStats {
    pub active_connections: usize,
    pub cached_connections: usize,
}

#[derive(Debug)]
struct PeerVerifier {
    key_store: Arc<RwLock<KeyStore>>,
}

impl rustls::client::danger::ServerCertVerifier for PeerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        // Calculate fingerprint
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(end_entity.as_ref());
        let fingerprint = format!("{:x}", hasher.finalize());

        let ks = self.key_store.read();

        if ks.is_trusted(&fingerprint) {
            return Ok(rustls::client::danger::ServerCertVerified::assertion());
        }

        if ks.is_pairing_mode() {
            info!(
                "Allowing unknown/blocked peer in PAIRING MODE: {}",
                fingerprint
            );
            return Ok(rustls::client::danger::ServerCertVerified::assertion());
        }

        if ks.is_blocked(&fingerprint) {
            warn!("Rejected BLOCKED peer: {}", fingerprint);
            return Err(rustls::Error::General("Peer is blocked".to_string()));
        }

        warn!("Allowing Unknown Certificate (Untrusted): {}", fingerprint);
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
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
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
        ]
    }
}

#[derive(Debug)]
struct ClientVerifier {
    key_store: Arc<RwLock<KeyStore>>,
}

impl rustls::server::danger::ClientCertVerifier for ClientVerifier {
    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
        // Calculate fingerprint
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(end_entity.as_ref());
        let fingerprint = format!("{:x}", hasher.finalize());

        let ks = self.key_store.read();

        if ks.is_trusted(&fingerprint) {
            return Ok(rustls::server::danger::ClientCertVerified::assertion());
        }

        if ks.is_pairing_mode() {
            info!(
                "Allowing unknown/blocked client in PAIRING MODE: {}",
                fingerprint
            );
            return Ok(rustls::server::danger::ClientCertVerified::assertion());
        }

        if ks.is_blocked(&fingerprint) {
            warn!("Rejected BLOCKED client: {}", fingerprint);
            return Err(rustls::Error::General("Client is blocked".to_string()));
        }

        warn!(
            "Allowing Unknown Client Certificate (Untrusted): {}",
            fingerprint
        );
        Ok(rustls::server::danger::ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
        ]
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
}
