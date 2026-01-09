use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConnectedError {
    #[error("Initialization error: {0}")]
    InitializationError(String),

    #[error("Discovery error: {0}")]
    Discovery(String),

    #[error("mDNS error: {0}")]
    Mdns(#[from] mdns_sd::Error),

    #[error("QUIC connection error: {0}")]
    QuicConnection(#[from] quinn::ConnectionError),

    #[error("QUIC connect error: {0}")]
    QuicConnect(#[from] quinn::ConnectError),

    #[error("QUIC write error: {0}")]
    QuicWrite(#[from] quinn::WriteError),

    #[error("QUIC read error: {0}")]
    QuicRead(#[from] quinn::ReadExactError),

    #[error("QUIC read to end error: {0}")]
    QuicReadToEnd(#[from] quinn::ReadToEndError),

    #[error("QUIC stream closed: {0}")]
    QuicClosedStream(#[from] quinn::ClosedStream),

    #[error("TLS error: {0}")]
    Tls(#[from] rustls::Error),

    #[error("Certificate generation error: {0}")]
    CertGen(#[from] rcgen::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Invalid address: {0}")]
    InvalidAddress(String),

    #[error("Timeout: {0}")]
    Timeout(String),

    #[error("Not initialized")]
    NotInitialized,

    #[error("Already running")]
    AlreadyRunning,

    #[error("Ping failed: {0}")]
    PingFailed(String),

    #[error("Connection error: {0}")]
    Connection(String),

    #[error("File transfer rejected: {0}")]
    TransferRejected(String),

    #[error("File transfer failed: {0}")]
    TransferFailed(String),

    #[error("Checksum mismatch")]
    ChecksumMismatch,

    #[error("Protocol error: {0}")]
    Protocol(String),

    #[error("Pairing failed: {0}")]
    PairingFailed(String),

    #[error("Peer not trusted")]
    PeerNotTrusted,

    #[error("Peer is blocked")]
    PeerBlocked,

    #[error("Filesystem error: {0}")]
    Filesystem(String),
}

pub type Result<T> = std::result::Result<T, ConnectedError>;
