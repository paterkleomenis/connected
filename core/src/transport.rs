use crate::error::{ConnectedError, Result};
use crate::security::KeyStore;
use crate::telephony::TelephonyMessage;
use parking_lot::RwLock;
use quinn::{
    ClientConfig, Connection, Endpoint, RecvStream, SendStream, ServerConfig, TransportConfig,
    VarInt,
};

use rustls::DistinguishedName;
use rustls::pki_types::CertificateDer;
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
        listening_port: u16,
    },
    HandshakeAck {
        device_id: String,
        device_name: String,
    },
    Clipboard {
        text: String,
    },
    FileTransfer,
    /// Sent when a device unpairs/forgets/blocks another device
    DeviceUnpaired {
        device_id: String,
        reason: UnpairReason,
    },
    MediaControl(MediaControlMessage),
    /// Telephony messages (SMS, calls, contacts)
    Telephony(TelephonyMessage),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MediaControlMessage {
    Command(MediaCommand),
    StateUpdate(MediaState),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MediaCommand {
    Play,
    Pause,
    PlayPause,
    Next,
    Previous,
    Stop,
    VolumeUp,
    VolumeDown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MediaState {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub playing: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum UnpairReason {
    Unpaired,
    Forgotten,
}

struct ConnectionCache {
    connections: HashMap<SocketAddr, CachedConnection>,
}

struct CachedConnection {
    connection: Connection,
    last_used: std::time::Instant,
}

#[derive(Clone)]
struct TransportHandlers {
    message_tx: mpsc::UnboundedSender<(SocketAddr, String, Message, Option<SendStream>)>,
    file_stream_tx: mpsc::UnboundedSender<(String, SendStream, RecvStream)>,
    fs_stream_tx: mpsc::UnboundedSender<(String, SendStream, RecvStream)>,
}

impl ConnectionCache {
    fn new() -> Self {
        Self {
            connections: HashMap::new(),
        }
    }

    fn canonicalize_addr(addr: &SocketAddr) -> SocketAddr {
        match addr.ip() {
            std::net::IpAddr::V6(v6) => {
                if let Some(v4) = v6.to_ipv4() {
                    SocketAddr::new(std::net::IpAddr::V4(v4), addr.port())
                } else {
                    *addr
                }
            }
            _ => *addr,
        }
    }

    fn get(&mut self, addr: &SocketAddr) -> Option<Connection> {
        let key = Self::canonicalize_addr(addr);
        if let Some(cached) = self.connections.get_mut(&key) {
            if cached.connection.close_reason().is_none() {
                cached.last_used = std::time::Instant::now();
                return Some(cached.connection.clone());
            } else {
                self.connections.remove(&key);
            }
        }
        None
    }

    fn insert(&mut self, addr: SocketAddr, connection: Connection) {
        let key = Self::canonicalize_addr(&addr);
        let cutoff = std::time::Instant::now() - Duration::from_secs(300);
        self.connections
            .retain(|_, v| v.last_used > cutoff && v.connection.close_reason().is_none());

        self.connections.insert(
            key,
            CachedConnection {
                connection,
                last_used: std::time::Instant::now(),
            },
        );
    }

    fn register_alias(&mut self, original_addr: SocketAddr, alias_addr: SocketAddr) {
        let key = Self::canonicalize_addr(&original_addr);
        if let Some(cached) = self.connections.get(&key) {
            let conn = cached.connection.clone();
            let alias_key = Self::canonicalize_addr(&alias_addr);
            self.connections.insert(
                alias_key,
                CachedConnection {
                    connection: conn,
                    last_used: std::time::Instant::now(),
                },
            );
            debug!("Aliased connection {} -> {}", alias_addr, original_addr);
        } else {
            warn!(
                "Could not alias {} -> {}: original not found",
                alias_addr, original_addr
            );
        }
    }

    fn remove(&mut self, addr: &SocketAddr) -> Option<Connection> {
        let key = Self::canonicalize_addr(addr);
        self.connections.remove(&key).map(|c| c.connection)
    }
}

pub struct QuicTransport {
    endpoint: Endpoint,
    local_id: String,
    client_config: ClientConfig,
    client_config_allow_unknown: ClientConfig,
    connection_cache: Arc<RwLock<ConnectionCache>>,
    handlers: Arc<RwLock<Option<TransportHandlers>>>,
    key_store: Arc<RwLock<KeyStore>>,
}

impl QuicTransport {
    pub async fn new(
        bind_addr: SocketAddr,
        local_id: String,
        key_store: Arc<RwLock<KeyStore>>,
    ) -> Result<Self> {
        let (server_config, _) = Self::create_server_config(&key_store)?;
        let client_config = Self::create_client_config(&key_store, false)?;
        let client_config_allow_unknown = Self::create_client_config(&key_store, true)?;

        let mut endpoint = Endpoint::server(server_config, bind_addr)?;
        endpoint.set_default_client_config(client_config.clone());

        info!("QUIC transport listening on {}", bind_addr);

        Ok(Self {
            endpoint,
            local_id,
            client_config,
            client_config_allow_unknown,
            connection_cache: Arc::new(RwLock::new(ConnectionCache::new())),
            handlers: Arc::new(RwLock::new(None)),
            key_store: key_store.clone(),
        })
    }

    fn spawn_connection_handler(&self, connection: Connection, remote_addr: SocketAddr) {
        let handlers = { self.handlers.read().clone() };

        let Some(handlers) = handlers else {
            debug!(
                "No transport handlers registered; skipping connection handler for {}",
                remote_addr
            );
            return;
        };

        let local_id = self.local_id.clone();
        let key_store = self.key_store.clone();

        tokio::spawn(async move {
            if let Err(e) = Self::handle_connection(
                connection,
                remote_addr,
                handlers.message_tx,
                handlers.file_stream_tx,
                handlers.fs_stream_tx,
                local_id,
                key_store,
            )
            .await
            {
                warn!("Connection handler error: {}", e);
            }
        });
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

    fn create_client_config(
        key_store: &Arc<RwLock<KeyStore>>,
        allow_unknown: bool,
    ) -> Result<ClientConfig> {
        let ks = key_store.read();
        let cert = ks.get_cert();
        let key = ks.get_key();

        let mut client_crypto = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(PeerVerifier {
                key_store: key_store.clone(),
                allow_unknown,
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

        info!("Establishing new QUIC connection to {}", addr);

        let connecting =
            self.endpoint
                .connect_with(self.client_config.clone(), addr, "connected.local")?;

        let connection = timeout(CONNECT_TIMEOUT, connecting)
            .await
            .map_err(|_| {
                warn!("Connection to {} timed out after {:?}", addr, CONNECT_TIMEOUT);
                ConnectedError::Timeout(format!(
                    "Connection to {} timed out after {:?}. Ensure the device is online and reachable.",
                    addr, CONNECT_TIMEOUT
                ))
            })?
            .map_err(|e| {
                warn!("Connection to {} failed: {}", addr, e);
                ConnectedError::Connection(format!("Failed to connect to {}: {}", addr, e))
            })?;

        info!("Connected to {} (RTT: {:?})", addr, connection.rtt());

        {
            let mut cache = self.connection_cache.write();
            cache.insert(addr, connection.clone());
        }

        self.spawn_connection_handler(connection.clone(), addr);

        Ok(connection)
    }

    pub async fn connect_allow_unknown(&self, addr: SocketAddr) -> Result<Connection> {
        info!(
            "Establishing new QUIC connection (allow unknown) to {}",
            addr
        );

        let connecting = self.endpoint.connect_with(
            self.client_config_allow_unknown.clone(),
            addr,
            "connected.local",
        )?;

        let connection = timeout(CONNECT_TIMEOUT, connecting)
            .await
            .map_err(|_| {
                warn!(
                    "Connection to {} timed out after {:?}",
                    addr, CONNECT_TIMEOUT
                );
                ConnectedError::Timeout(format!(
                    "Connection to {} timed out after {:?}. Ensure the device is online and reachable.",
                    addr, CONNECT_TIMEOUT
                ))
            })?
            .map_err(|e| {
                warn!("Connection to {} failed: {}", addr, e);
                ConnectedError::Connection(format!("Failed to connect to {}: {}", addr, e))
            })?;

        info!("Connected to {} (RTT: {:?})", addr, connection.rtt());

        self.spawn_connection_handler(connection.clone(), addr);

        Ok(connection)
    }

    pub fn invalidate_connection(&self, addr: &SocketAddr) {
        self.invalidate_connection_with_reason(addr, b"unpaired");
    }

    pub fn invalidate_connection_with_reason(&self, addr: &SocketAddr, reason: &[u8]) {
        let mut cache = self.connection_cache.write();
        match cache.remove(addr) {
            Some(connection) => {
                // Close the QUIC connection gracefully
                connection.close(VarInt::from_u32(0), reason);
                info!("Closed and invalidated connection to {}", addr);
            }
            _ => {
                debug!("No cached connection to invalidate for {}", addr);
            }
        }
    }

    pub fn invalidate_connection_by_fingerprint(&self, fingerprint: &str) {
        self.invalidate_connection_by_fingerprint_with_reason(fingerprint, b"unpaired");
    }

    pub fn invalidate_connection_by_fingerprint_with_reason(
        &self,
        fingerprint: &str,
        reason: &[u8],
    ) {
        let mut cache = self.connection_cache.write();
        let mut to_remove = Vec::new();

        for (addr, cached) in cache.connections.iter() {
            if let Some(fp) = Self::get_peer_fingerprint(&cached.connection)
                && fp == fingerprint
            {
                to_remove.push(*addr);
            }
        }

        for addr in to_remove {
            if let Some(cached) = cache.connections.remove(&addr) {
                cached.connection.close(VarInt::from_u32(0), reason);
                info!(
                    "Closed and invalidated connection to {} (fingerprint match)",
                    addr
                );
            }
        }
    }

    pub fn register_connection_alias(&self, original: SocketAddr, alias: SocketAddr) {
        self.connection_cache
            .write()
            .register_alias(original, alias);
    }

    pub async fn get_connection_fingerprint(&self, addr: SocketAddr) -> Option<String> {
        let cache = self.connection_cache.read();
        let key = ConnectionCache::canonicalize_addr(&addr);
        if let Some(cached) = cache.connections.get(&key) {
            let close_reason = cached.connection.close_reason();
            if close_reason.is_none() {
                let fp = Self::get_peer_fingerprint(&cached.connection);
                if fp.is_some() {
                    debug!(
                        "Got fingerprint for {}: {:?}",
                        addr,
                        fp.as_ref().map(|s| &s[..16.min(s.len())])
                    );
                } else {
                    warn!(
                        "Connection to {} exists but could not get peer fingerprint (no peer identity?)",
                        addr
                    );
                }
                return fp;
            } else {
                warn!(
                    "Connection to {} is closed (reason: {:?}), cannot get fingerprint",
                    addr, close_reason
                );
            }
        } else {
            warn!(
                "No cached connection found for {} (cache has {} connections)",
                addr,
                cache.connections.len()
            );
        }
        None
    }

    pub fn cleanup_stale_connections(&self) {
        let mut cache = self.connection_cache.write();
        let before = cache.connections.len();
        let cutoff = std::time::Instant::now() - Duration::from_secs(300);
        cache
            .connections
            .retain(|_, v| v.last_used > cutoff && v.connection.close_reason().is_none());
        let after = cache.connections.len();
        if before != after {
            debug!("Cleaned up {} stale connections", before - after);
        }
    }

    pub async fn send_ping(&self, target_addr: SocketAddr) -> Result<Duration> {
        let start = std::time::Instant::now();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
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

    pub const STREAM_TYPE_FS: u8 = 3;

    pub async fn start_server(
        &self,
        message_tx: mpsc::UnboundedSender<(SocketAddr, String, Message, Option<SendStream>)>,
        file_stream_tx: mpsc::UnboundedSender<(String, SendStream, RecvStream)>,
        fs_stream_tx: mpsc::UnboundedSender<(String, SendStream, RecvStream)>,
    ) -> Result<()> {
        {
            let mut handlers = self.handlers.write();
            *handlers = Some(TransportHandlers {
                message_tx: message_tx.clone(),
                file_stream_tx: file_stream_tx.clone(),
                fs_stream_tx: fs_stream_tx.clone(),
            });
        }

        let endpoint = self.endpoint.clone();
        let local_id = self.local_id.clone();
        let connection_cache = self.connection_cache.clone();
        let key_store = self.key_store.clone();

        tokio::spawn(async move {
            info!("QUIC server started, waiting for connections");

            while let Some(incoming) = endpoint.accept().await {
                let tx = message_tx.clone();
                let f_tx = file_stream_tx.clone();
                let fs_tx = fs_stream_tx.clone();
                let id = local_id.clone();
                let cache = connection_cache.clone();
                let ks = key_store.clone();

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

                                // Cache the incoming connection so we can reuse it for outgoing requests
                                {
                                    let mut c = cache.write();
                                    c.insert(remote_addr, connection.clone());
                                }

                                if let Err(e) = Self::handle_connection(
                                    connection,
                                    remote_addr,
                                    tx,
                                    f_tx,
                                    fs_tx,
                                    id,
                                    ks,
                                )
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
        message_tx: mpsc::UnboundedSender<(SocketAddr, String, Message, Option<SendStream>)>,
        file_stream_tx: mpsc::UnboundedSender<(String, SendStream, RecvStream)>,
        fs_stream_tx: mpsc::UnboundedSender<(String, SendStream, RecvStream)>,
        local_id: String,
        key_store: Arc<RwLock<KeyStore>>,
    ) -> Result<()> {
        let mut fingerprint =
            Self::get_peer_fingerprint(&connection).unwrap_or_else(|| "unknown".to_string());

        loop {
            match connection.accept_bi().await {
                Ok((mut send, mut recv)) => {
                    // Read Stream Type

                    let mut type_buf = [0u8; 1];

                    if recv.read_exact(&mut type_buf).await.is_err() {
                        continue;
                    }

                    if fingerprint == "unknown"
                        && let Some(fp) = Self::get_peer_fingerprint(&connection)
                    {
                        fingerprint = fp;
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

                                    // Send Length + Data (stream type already read on this stream)
                                    send.write_all(&len_bytes).await?;

                                    send.write_all(&pong_data).await?;

                                    send.finish()?;
                                }

                                Message::Handshake { .. } => {
                                    // Pass send stream for Handshake so ack can be sent on same stream
                                    let _ = message_tx.send((
                                        remote_addr,
                                        fingerprint.clone(),
                                        message,
                                        Some(send),
                                    ));
                                }
                                _ => {
                                    let _ = message_tx.send((
                                        remote_addr,
                                        fingerprint.clone(),
                                        message,
                                        None,
                                    ));
                                }
                            }
                        }

                        Self::STREAM_TYPE_FILE => {
                            // Hand off the stream to the file handler

                            info!("Received File Stream from {}", fingerprint);

                            let _ = file_stream_tx.send((fingerprint.clone(), send, recv));
                        }

                        Self::STREAM_TYPE_FS => {
                            info!("Received Filesystem Stream from {}", fingerprint);
                            let _ = fs_stream_tx.send((fingerprint.clone(), send, recv));
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

                    if reason.reason == b"unpaired".as_slice()
                        || reason.reason == b"forgotten".as_slice()
                    {
                        let unpair_reason = if reason.reason == b"forgotten".as_slice() {
                            UnpairReason::Forgotten
                        } else {
                            UnpairReason::Unpaired
                        };

                        let reason_str = if unpair_reason == UnpairReason::Forgotten {
                            "forgotten"
                        } else {
                            "unpaired"
                        };

                        let device_id_opt = {
                            let ks = key_store.read();
                            ks.get_peer_info(&fingerprint).and_then(|p| p.device_id)
                        };

                        if let Some(device_id) = device_id_opt {
                            info!(
                                "Peer {} ({}) disconnected with '{}' reason. Triggering unpair/forget.",
                                device_id, fingerprint, reason_str
                            );
                            let _ = message_tx.send((
                                remote_addr,
                                fingerprint.clone(),
                                Message::DeviceUnpaired {
                                    device_id,
                                    reason: unpair_reason,
                                },
                                None,
                            ));
                        } else {
                            warn!(
                                "Peer closed with '{}' but device_id not found for fingerprint {}",
                                reason_str, fingerprint
                            );
                        }
                    }

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
        let identity = match connection.peer_identity() {
            Some(id) => id,
            None => {
                debug!("No peer identity available for connection");
                return None;
            }
        };
        let certs = match identity.downcast_ref::<Vec<rustls::pki_types::CertificateDer>>() {
            Some(c) => c,
            None => {
                debug!("Could not downcast peer identity to certificate chain");
                return None;
            }
        };
        if let Some(cert) = certs.first() {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(cert.as_ref());
            Some(format!("{:x}", hasher.finalize()))
        } else {
            debug!("Certificate chain is empty");
            None
        }
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.endpoint.local_addr().map_err(ConnectedError::Io)
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

#[derive(Debug)]
struct PeerVerifier {
    key_store: Arc<RwLock<KeyStore>>,
    allow_unknown: bool,
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
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(end_entity.as_ref());
        let fingerprint = format!("{:x}", hasher.finalize());

        let ks = self.key_store.read();

        // Always reject blocked peers, regardless of pairing mode
        // if ks.is_blocked(&fingerprint) {
        //    warn!("Rejected BLOCKED peer: {}", fingerprint);
        //    return Err(rustls::Error::General("Peer is blocked".to_string()));
        // }

        if ks.is_trusted(&fingerprint) {
            debug!("Accepted trusted peer: {}", fingerprint);
            return Ok(rustls::client::danger::ServerCertVerified::assertion());
        }

        // Forgotten peers can connect but will need to go through pairing request
        // Allow connection so handshake message can be processed
        if ks.is_forgotten(&fingerprint) {
            info!(
                "Allowing FORGOTTEN peer (will require re-pairing): {}",
                fingerprint
            );
            return Ok(rustls::client::danger::ServerCertVerified::assertion());
        }

        // Only allow unknown peers if pairing mode is enabled
        // or if this connection explicitly allows unknown peers (e.g. file transfer).
        if self.allow_unknown || ks.is_pairing_mode() {
            let reason = if self.allow_unknown {
                "ALLOW_UNKNOWN"
            } else {
                "PAIRING_MODE"
            };
            info!("Allowing unknown peer ({}): {}", reason, fingerprint);
            return Ok(rustls::client::danger::ServerCertVerified::assertion());
        }

        // Reject unknown peers when not in pairing mode
        warn!(
            "Rejected unknown peer (not in pairing mode): {}",
            fingerprint
        );
        Err(rustls::Error::General(
            "Unknown peer - enable pairing mode to connect".to_string(),
        ))
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[derive(Debug)]
struct ClientVerifier {
    key_store: Arc<RwLock<KeyStore>>,
}

impl rustls::server::danger::ClientCertVerifier for ClientVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        true
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(end_entity.as_ref());
        let fingerprint = format!("{:x}", hasher.finalize());

        let ks = self.key_store.read();

        // Always reject blocked peers
        // if ks.is_blocked(&fingerprint) {
        //    warn!("Rejected BLOCKED client: {}", fingerprint);
        //    return Err(rustls::Error::General("Client is blocked".to_string()));
        // }

        if ks.is_trusted(&fingerprint) {
            debug!("Accepted trusted client: {}", fingerprint);
            return Ok(rustls::server::danger::ClientCertVerified::assertion());
        }

        // Forgotten clients can connect but will trigger pairing request
        if ks.is_forgotten(&fingerprint) {
            info!(
                "Allowing FORGOTTEN client (will require re-pairing): {}",
                fingerprint
            );
            return Ok(rustls::server::danger::ClientCertVerified::assertion());
        }

        // Allow unknown clients to connect so we can show pairing dialog
        // The trust decision is made at the application layer (user clicks Trust/Reject)
        // NOT at the TLS layer. This enables incoming pairing requests to be received
        // and displayed to the user regardless of pairing mode.
        info!(
            "Allowing unknown client to connect (will show pairing request): {}",
            fingerprint
        );
        Ok(rustls::server::danger::ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
}
