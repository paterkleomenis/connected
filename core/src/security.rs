use crate::error::{ConnectedError, Result};
use rcgen::{CertifiedKey, generate_simple_self_signed};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

/// Restrict a file or directory so that only the current user can access it.
///
/// On Unix this is handled via `chmod 0600` (files) / `chmod 0700` (dirs) elsewhere.
/// On Windows we use the built-in `icacls` command to:
///   1. Remove all inherited ACEs.
///   2. Grant full control to the current user (%USERNAME%) with inheritance
///      flags (OI)(CI) so child files/subdirectories inherit the ACE.
///   3. Grant full control to SYSTEM so that OS services (backup, indexing,
///      etc.) continue to function.
/// This avoids adding a heavy `windows-sys` dependency while achieving
/// owner-only access semantics equivalent to Unix `0600`/`0700`.
#[cfg(windows)]
fn restrict_path_to_current_user(path: &std::path::Path) {
    // Best-effort: log warnings on failure but don't abort — the application
    // should still function even if ACLs cannot be set (e.g. network drives).
    let path_str = match path.to_str() {
        Some(s) => s.to_string(),
        None => {
            warn!(
                "Cannot restrict ACLs for {:?}: path is not valid UTF-8",
                path
            );
            return;
        }
    };

    let username = match std::env::var("USERNAME") {
        Ok(u) if !u.is_empty() => u,
        _ => {
            warn!("Cannot restrict ACLs: %USERNAME% environment variable not set");
            return;
        }
    };

    // Check whether we can actually read the path before modifying ACLs.
    // If we can't even stat it, there is no point stripping inheritance
    // (which would make things worse).
    if let Err(e) = std::fs::metadata(path) {
        warn!(
            "Cannot access {:?} to restrict ACLs ({}); skipping ACL modification",
            path, e
        );
        return;
    }

    let is_dir = path.is_dir();

    // Step 1: Grant full control to the current user BEFORE removing
    // inheritance.  This ensures we never leave the object in a state
    // with an empty DACL (which would lock everyone out).
    //
    // For directories we use (OI)(CI)(F) so that child files and
    // subdirectories inherit the ACE.  For plain files inheritance
    // flags are unnecessary, so we use just (F).
    let ace = if is_dir {
        format!("{}:(OI)(CI)(F)", username)
    } else {
        format!("{}:(F)", username)
    };

    let grant = std::process::Command::new("icacls")
        .arg(&path_str)
        .arg("/grant")
        .arg(&ace)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    match grant {
        Ok(status) if status.success() => {}
        Ok(status) => {
            warn!(
                "icacls /grant failed for {:?} (exit code {:?}); skipping further ACL changes",
                path,
                status.code()
            );
            return;
        }
        Err(e) => {
            warn!(
                "Failed to run icacls /grant for {:?}: {}; skipping further ACL changes",
                path, e
            );
            return;
        }
    }

    // Step 2: Now that we have an explicit ACE, safely remove inherited ACEs.
    let strip = std::process::Command::new("icacls")
        .arg(&path_str)
        .args(["/inheritance:r"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    match strip {
        Ok(status) if status.success() => {}
        Ok(status) => {
            warn!(
                "icacls /inheritance:r failed for {:?} (exit code {:?})",
                path,
                status.code()
            );
            // The explicit grant above still tightened access, so this is
            // non-fatal — inherited ACEs just remain.
        }
        Err(e) => {
            warn!("Failed to run icacls /inheritance:r for {:?}: {}", path, e);
        }
    }

    // Step 3: Also grant SYSTEM full control so that OS services (backup,
    // search indexing, Windows Update, etc.) continue to function.
    let system_ace = if is_dir {
        "SYSTEM:(OI)(CI)(F)"
    } else {
        "SYSTEM:(F)"
    };

    let grant_system = std::process::Command::new("icacls")
        .arg(&path_str)
        .arg("/grant")
        .arg(system_ace)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    match grant_system {
        Ok(status) if status.success() => {
            // Success — SYSTEM access restored.
        }
        Ok(status) => {
            warn!(
                "icacls /grant SYSTEM failed for {:?} (exit code {:?})",
                path,
                status.code()
            );
        }
        Err(e) => {
            warn!("Failed to run icacls /grant SYSTEM for {:?}: {}", path, e);
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum PeerStatus {
    /// Device is trusted and can connect/transfer freely
    Trusted,
    /// Device is unpaired - keys preserved but disconnected/inactive
    Unpaired,
    /// Device was forgotten - completely removed, requires full pairing request flow
    Forgotten,
    /// Device is blocked - all connections and data exchange are rejected
    Blocked,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PeerInfo {
    pub fingerprint: String,
    pub status: PeerStatus,
    pub device_id: Option<String>,
    pub name: Option<String>,
    pub last_seen: u64,
}

#[derive(Serialize, Deserialize, Default, Debug)]
struct KnownPeers {
    // Fingerprint -> PeerInfo
    peers: HashMap<String, PeerInfo>,
}

#[derive(Debug)]
pub struct KeyStore {
    cert: CertificateDer<'static>,
    key: PrivatePkcs8KeyDer<'static>,
    device_id: String,
    known_peers: KnownPeers,
    storage_dir: PathBuf,
    pairing_mode: bool,
    blocked_peers: std::collections::HashSet<String>,
}

impl KeyStore {
    pub fn new(custom_path: Option<PathBuf>) -> Result<Self> {
        let storage_dir = if let Some(p) = custom_path {
            p
        } else {
            dirs::config_dir()
                .ok_or_else(|| {
                    ConnectedError::InitializationError("Could not find config dir".to_string())
                })?
                .join("connected")
        };

        if !storage_dir.exists() {
            std::fs::create_dir_all(&storage_dir).map_err(ConnectedError::Io)?;
        }

        // Restrict the storage directory itself so other users cannot list or
        // read identity/peer files. On Unix this is 0700; on Windows we set an
        // owner-only ACL.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o700);
            if let Err(e) = std::fs::set_permissions(&storage_dir, perms) {
                warn!("Failed to set storage directory permissions: {}", e);
            }
        }
        #[cfg(windows)]
        {
            restrict_path_to_current_user(&storage_dir);
        }

        let (cert, key, device_id) = Self::load_or_create_identity(&storage_dir)?;
        let known_peers = Self::load_known_peers(&storage_dir)?;

        // Build blocked peers set from known peers for fast TLS-level lookups
        let blocked_peers: std::collections::HashSet<String> = known_peers
            .peers
            .iter()
            .filter(|(_, p)| p.status == PeerStatus::Blocked)
            .map(|(fp, _)| fp.clone())
            .collect();

        Ok(Self {
            cert,
            key,
            device_id,
            known_peers,
            storage_dir,
            pairing_mode: false,
            blocked_peers,
        })
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Verify identity file permissions on load and warn if too permissive
    #[cfg(unix)]
    fn check_identity_permissions(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(path) {
            let mode = metadata.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                warn!(
                    "Identity file {:?} has overly permissive permissions ({:o}). \
                     Expected 0600 (owner-only). Consider running: chmod 600 {:?}",
                    path, mode, path
                );
            }
        }
    }

    /// On Windows, verify/apply ACL restrictions on identity files.
    #[cfg(windows)]
    fn check_identity_permissions(path: &std::path::Path) {
        restrict_path_to_current_user(path);
    }

    /// No-op on platforms that are neither Unix nor Windows.
    #[cfg(not(any(unix, windows)))]
    fn check_identity_permissions(_path: &std::path::Path) {}

    fn load_or_create_identity(
        storage_dir: &std::path::Path,
    ) -> Result<(CertificateDer<'static>, PrivatePkcs8KeyDer<'static>, String)> {
        let identity_path = storage_dir.join("identity.json");

        if identity_path.exists() {
            return Self::load_identity_der(&identity_path);
        }

        info!("No identity found, generating new one...");
        let CertifiedKey { cert, signing_key } = generate_simple_self_signed(vec![
            "connected.local".to_string(),
            "localhost".to_string(),
        ])
        .map_err(|e| ConnectedError::InitializationError(e.to_string()))?;

        let cert_der = cert.der().to_vec();
        let key_der = signing_key.serialize_der();
        let device_id = uuid::Uuid::new_v4().to_string();

        // Save
        let persisted = PersistedIdentityDer {
            cert_der: cert_der.clone(),
            key_der: key_der.clone(),
            device_id: device_id.clone(),
        };
        let data = serde_json::to_vec_pretty(&persisted)
            .map_err(|e| ConnectedError::InitializationError(e.to_string()))?;

        // Atomic write: write to temp file, set permissions, fsync, then rename.
        // This prevents a half-written identity file if the process crashes mid-write.
        let tmp_path = identity_path.with_extension("json.tmp");
        std::fs::write(&tmp_path, &data).map_err(ConnectedError::Io)?;

        // Restrict permissions to owner-only to protect the private key
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&tmp_path, perms).map_err(ConnectedError::Io)?;
        }
        #[cfg(windows)]
        {
            restrict_path_to_current_user(&tmp_path);
        }

        // fsync to ensure data is durable before rename
        {
            let f = std::fs::File::open(&tmp_path).map_err(ConnectedError::Io)?;
            f.sync_all().map_err(ConnectedError::Io)?;
        }

        std::fs::rename(&tmp_path, &identity_path).map_err(ConnectedError::Io)?;

        Ok((
            CertificateDer::from(cert_der),
            PrivatePkcs8KeyDer::from(key_der),
            device_id,
        ))
    }

    fn load_identity_der(
        path: &std::path::Path,
    ) -> Result<(CertificateDer<'static>, PrivatePkcs8KeyDer<'static>, String)> {
        // Check permissions on existing identity file
        #[cfg(unix)]
        Self::check_identity_permissions(path);

        let data = std::fs::read(path).map_err(ConnectedError::Io)?;
        let mut persisted: PersistedIdentityDer = serde_json::from_slice(&data).map_err(|e| {
            ConnectedError::InitializationError(format!("Failed to parse identity: {}", e))
        })?;

        // Check if device_id was missing or empty (legacy file without the field).
        // The serde default now produces an empty string as a sentinel rather than
        // a random UUID, so we can detect the legacy case without double-parsing.
        let had_device_id = !persisted.device_id.is_empty();

        if !had_device_id {
            // Derive a *deterministic* id from the certificate so that repeated
            // loads (e.g. concurrent processes, or a crash before the save below)
            // always produce the same value.
            persisted.device_id = deterministic_device_id(&persisted.cert_der);
            info!(
                "Legacy identity file missing device_id, persisting deterministic id: {}",
                persisted.device_id
            );
            let updated_data = serde_json::to_vec_pretty(&persisted)
                .map_err(|e| ConnectedError::InitializationError(e.to_string()))?;

            // Atomic write: write to temp file, set permissions, fsync, then rename.
            // This prevents corruption if the process crashes mid-write.
            let tmp_path = path.with_extension("json.tmp");
            std::fs::write(&tmp_path, &updated_data).map_err(ConnectedError::Io)?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = std::fs::Permissions::from_mode(0o600);
                let _ = std::fs::set_permissions(&tmp_path, perms);
            }

            {
                let f = std::fs::File::open(&tmp_path).map_err(ConnectedError::Io)?;
                f.sync_all().map_err(ConnectedError::Io)?;
            }

            std::fs::rename(&tmp_path, path).map_err(ConnectedError::Io)?;
        }

        Ok((
            CertificateDer::from(persisted.cert_der),
            PrivatePkcs8KeyDer::from(persisted.key_der),
            persisted.device_id,
        ))
    }

    fn load_known_peers(storage_dir: &std::path::Path) -> Result<KnownPeers> {
        let new_path = storage_dir.join("known_peers.json");

        if new_path.exists() {
            let data = std::fs::read(&new_path).map_err(ConnectedError::Io)?;
            match serde_json::from_slice(&data) {
                Ok(peers) => Ok(peers),
                Err(e) => {
                    warn!("Failed to parse known peers file: {}", e);
                    let ts = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    let corrupt_path = storage_dir.join(format!("known_peers.corrupt.{}.json", ts));
                    if let Err(err) = std::fs::rename(&new_path, &corrupt_path) {
                        warn!(
                            "Failed to backup corrupt peers file to {}: {}",
                            corrupt_path.display(),
                            err
                        );
                    }
                    Ok(KnownPeers::default())
                }
            }
        } else {
            Ok(KnownPeers::default())
        }
    }

    fn save_peers(&self) -> Result<()> {
        let data = serde_json::to_vec_pretty(&self.known_peers)
            .map_err(|e| ConnectedError::InitializationError(e.to_string()))?;

        // M4: Atomic write — write to temp file, fsync, then rename over the target
        let final_path = self.storage_dir.join("known_peers.json");
        let tmp_path = self.storage_dir.join("known_peers.json.tmp");

        std::fs::write(&tmp_path, &data).map_err(ConnectedError::Io)?;

        // Set owner-only permissions on the temp file before rename
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            if let Err(e) = std::fs::set_permissions(&tmp_path, perms) {
                warn!("Failed to set permissions on known_peers temp file: {}", e);
            }
        }
        #[cfg(windows)]
        {
            restrict_path_to_current_user(&tmp_path);
        }

        // fsync the temp file to ensure data is durable before rename
        {
            let f = std::fs::File::open(&tmp_path).map_err(ConnectedError::Io)?;
            f.sync_all().map_err(ConnectedError::Io)?;
        }

        std::fs::rename(&tmp_path, &final_path).map_err(ConnectedError::Io)?;
        Ok(())
    }

    pub fn trust_peer(
        &mut self,
        fingerprint: String,
        device_id: Option<String>,
        name: Option<String>,
    ) -> Result<()> {
        self.known_peers.peers.insert(
            fingerprint.clone(),
            PeerInfo {
                fingerprint,
                status: PeerStatus::Trusted,
                device_id,
                name,
                last_seen: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            },
        );
        self.save_peers()
    }

    /// Unpair a peer - keeps keys but marks as unpaired (inactive)
    /// Re-connecting acts as a fast-pairing (trust restoration)
    pub fn unpair_peer(&mut self, fingerprint: String) -> Result<()> {
        if let Some(peer) = self.known_peers.peers.get_mut(&fingerprint) {
            peer.status = PeerStatus::Unpaired;
            peer.last_seen = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            self.save_peers()
        } else {
            Ok(())
        }
    }

    /// Remove peer completely from known peers list
    /// This allows re-pairing without any restrictions
    pub fn remove_peer(&mut self, fingerprint: &str) -> Result<()> {
        if self.known_peers.peers.remove(fingerprint).is_some() {
            self.save_peers()
        } else {
            Ok(())
        }
    }

    /// Remove peer by device_id
    pub fn remove_peer_by_id(&mut self, device_id: &str) -> Result<()> {
        let fingerprint = self
            .known_peers
            .peers
            .values()
            .find(|p| p.device_id.as_deref() == Some(device_id))
            .map(|p| p.fingerprint.clone());

        if let Some(fp) = fingerprint {
            self.remove_peer(&fp)
        } else {
            Ok(())
        }
    }

    /// Block a peer - reject all connections and data exchange at TLS level
    pub fn block_peer(&mut self, fingerprint: String) -> Result<()> {
        if let Some(peer) = self.known_peers.peers.get_mut(&fingerprint) {
            peer.status = PeerStatus::Blocked;
            peer.last_seen = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
        } else {
            self.known_peers.peers.insert(
                fingerprint.clone(),
                PeerInfo {
                    fingerprint: fingerprint.clone(),
                    status: PeerStatus::Blocked,
                    device_id: None,
                    name: None,
                    last_seen: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                },
            );
        }
        self.blocked_peers.insert(fingerprint);
        self.save_peers()
    }

    /// Unblock a peer - removes blocked status (reverts to forgotten, requiring re-pairing)
    pub fn unblock_peer(&mut self, fingerprint: String) -> Result<()> {
        self.blocked_peers.remove(&fingerprint);
        if let Some(peer) = self.known_peers.peers.get_mut(&fingerprint) {
            peer.status = PeerStatus::Forgotten;
            peer.last_seen = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            self.save_peers()
        } else {
            Ok(())
        }
    }

    /// Check if a peer is blocked (fast lookup for TLS-level enforcement)
    pub fn is_blocked(&self, fingerprint: &str) -> bool {
        self.blocked_peers.contains(fingerprint)
    }

    /// Forget a peer - removes trust but keeps record to prevent auto-repairing
    /// The peer will need to go through pairing request flow again
    pub fn forget_peer(&mut self, fingerprint: String) -> Result<()> {
        if let Some(peer) = self.known_peers.peers.get_mut(&fingerprint) {
            peer.status = PeerStatus::Forgotten;
            peer.last_seen = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
        } else {
            // If not known, add as forgotten to prevent auto-trust
            self.known_peers.peers.insert(
                fingerprint.clone(),
                PeerInfo {
                    fingerprint,
                    status: PeerStatus::Forgotten,
                    device_id: None,
                    name: None,
                    last_seen: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                },
            );
        }
        self.save_peers()
    }

    /// Forget peer by device_id
    pub fn forget_peer_by_id(&mut self, device_id: &str) -> Result<()> {
        let fingerprint = self
            .known_peers
            .peers
            .values()
            .find(|p| p.device_id.as_deref() == Some(device_id))
            .map(|p| p.fingerprint.clone());

        if let Some(fp) = fingerprint {
            self.forget_peer(fp)
        } else {
            Ok(())
        }
    }

    pub fn get_cert(&self) -> CertificateDer<'static> {
        self.cert.clone()
    }

    pub fn get_key(&self) -> PrivatePkcs8KeyDer<'static> {
        self.key.clone_key()
    }

    pub fn is_trusted(&self, fingerprint: &str) -> bool {
        if let Some(peer) = self.known_peers.peers.get(fingerprint) {
            return peer.status == PeerStatus::Trusted;
        }
        false
    }

    pub fn is_unpaired(&self, fingerprint: &str) -> bool {
        if let Some(peer) = self.known_peers.peers.get(fingerprint) {
            return peer.status == PeerStatus::Unpaired;
        }
        false
    }

    /// Check if peer was forgotten (requires re-pairing with user confirmation)
    pub fn is_forgotten(&self, fingerprint: &str) -> bool {
        if let Some(peer) = self.known_peers.peers.get(fingerprint) {
            return peer.status == PeerStatus::Forgotten;
        }
        false
    }

    pub fn get_blocked_peers(&self) -> Vec<PeerInfo> {
        self.known_peers
            .peers
            .values()
            .filter(|p| p.status == PeerStatus::Blocked)
            .cloned()
            .collect()
    }

    /// Check if peer should trigger a pairing request (unknown or forgotten)
    pub fn needs_pairing_request(&self, fingerprint: &str) -> bool {
        match self.known_peers.peers.get(fingerprint) {
            None => true, // Unknown peer
            Some(peer) => peer.status == PeerStatus::Forgotten,
        }
    }

    pub fn get_peer_info(&self, fingerprint: &str) -> Option<PeerInfo> {
        self.known_peers.peers.get(fingerprint).cloned()
    }

    pub fn get_peer_name(&self, fingerprint: &str) -> Option<String> {
        self.known_peers
            .peers
            .get(fingerprint)
            .and_then(|p| p.name.clone())
    }

    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&self.cert);
        format!("{:x}", hasher.finalize())
    }

    // Pairing Mode Controls
    pub fn set_pairing_mode(&mut self, enabled: bool) {
        self.pairing_mode = enabled;
        info!("Pairing mode set to: {}", enabled);
    }

    pub fn is_pairing_mode(&self) -> bool {
        self.pairing_mode
    }

    pub fn get_trusted_peers(&self) -> Vec<PeerInfo> {
        self.known_peers
            .peers
            .values()
            .filter(|p| p.status == PeerStatus::Trusted)
            .cloned()
            .collect()
    }

    pub fn get_forgotten_peers(&self) -> Vec<PeerInfo> {
        self.known_peers
            .peers
            .values()
            .filter(|p| p.status == PeerStatus::Forgotten)
            .cloned()
            .collect()
    }

    pub fn get_all_known_peers(&self) -> Vec<PeerInfo> {
        self.known_peers.peers.values().cloned().collect()
    }
}

#[derive(Serialize, Deserialize)]
struct PersistedIdentityDer {
    cert_der: Vec<u8>,
    key_der: Vec<u8>,
    /// Legacy identity files may lack this field.  The serde default produces an
    /// empty string as a sentinel; `load_identity_der` then generates a
    /// *deterministic* ID from the certificate hash and persists it.
    ///
    /// Previous code used `uuid::Uuid::new_v4()` here, which meant every
    /// deserialization of a legacy file produced a *different* random ID — a
    /// correctness bug if the save-back ever failed or if the file was read
    /// concurrently.
    #[serde(default)]
    device_id: String,
}

/// Derive a stable, deterministic device-id from the certificate DER bytes.
/// Uses UUID v5 (SHA-1 namespace hash) so the same certificate always yields
/// the same id, even across multiple deserializations before the value is
/// persisted.
fn deterministic_device_id(cert_der: &[u8]) -> String {
    // Use the UUID DNS namespace as a convenient, well-known namespace UUID.
    let namespace = uuid::Uuid::NAMESPACE_DNS;
    uuid::Uuid::new_v5(&namespace, cert_der).to_string()
}
