use connected_core::security::KeyStore;
use connected_core::{ConnectedClient, DeviceType};

#[tokio::test]
async fn test_client_initialization() {
    let device_name = String::from("Test-Desktop");
    let port = 45000;

    let client = ConnectedClient::new(device_name.clone(), DeviceType::Unknown, port, None).await;

    assert!(client.is_ok(), "Client failed to initialize");
    let client = client.unwrap();

    let local_device = client.local_device();
    assert_eq!(local_device.name, device_name);
    assert!(!local_device.id.is_empty(), "device id must be generated");
    assert_ne!(local_device.port, 0, "listening port must be bound");

    client.shutdown().await;
}

/// A second client on the same storage dir must load the SAME identity
/// (fingerprint/device id) — this is what makes pairing survive restarts.
#[tokio::test]
async fn test_client_identity_persists_across_restart() {
    let storage = std::env::temp_dir().join(format!(
        "connected-test-identity-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&storage).expect("create storage dir");

    // Use distinct ports so two sequential clients don't fight over a socket.
    let first = ConnectedClient::new(
        "Restart-A".to_string(),
        DeviceType::Linux,
        45001,
        Some(storage.clone()),
    )
    .await
    .expect("first client init");

    let fp_first = first.get_fingerprint();
    let id_first = first.local_device().id;
    first.shutdown().await;
    // Give the OS a moment to release the UDP socket.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let second = ConnectedClient::new(
        "Restart-B".to_string(),
        DeviceType::Linux,
        45002,
        Some(storage.clone()),
    )
    .await
    .expect("second client init");

    assert_eq!(
        second.get_fingerprint(),
        fp_first,
        "identity (cert fingerprint) must persist across restarts"
    );
    assert_eq!(
        second.local_device().id,
        id_first,
        "device id must persist across restarts"
    );
    second.shutdown().await;

    let _ = std::fs::remove_dir_all(&storage);
}

/// Trust-state lifecycle against a REAL keystore on disk: trust → trusted,
/// unpair → unpaired-but-remembered (prevents silent re-pairing), block →
/// rejected at TLS layer. This exercises the same KeyStore paths the
/// pairing handlers use.
#[test]
fn test_keystore_trust_lifecycle() {
    let storage = std::env::temp_dir().join(format!(
        "connected-test-keystore-{}",
        uuid::Uuid::new_v4().simple()
    ));

    let mut ks = KeyStore::new(Some(storage.clone())).expect("keystore init");
    let peer_fp = format!("{:064x}", 0xABCDu64); // arbitrary but stable fingerprint

    // Fresh keystore: unknown peer is neither trusted nor blocked.
    assert!(!ks.is_trusted(&peer_fp));
    assert!(!ks.is_blocked(&peer_fp));
    assert!(!ks.is_unpaired(&peer_fp));

    // Trust → trusted with metadata attached.
    ks.trust_peer(
        peer_fp.clone(),
        Some("peer-device".into()),
        Some("Peer Phone".into()),
    )
    .expect("trust_peer");
    assert!(ks.is_trusted(&peer_fp));
    assert_eq!(ks.get_peer_name(&peer_fp).as_deref(), Some("Peer Phone"));

    // Unpair keeps the record (prevents auto-re-pairing) but drops trust.
    ks.unpair_peer(peer_fp.clone()).expect("unpair_peer");
    assert!(!ks.is_trusted(&peer_fp));
    assert!(ks.is_unpaired(&peer_fp));

    // Blocked peers are always rejected regardless of other state.
    ks.block_peer(peer_fp.clone()).expect("block_peer");
    assert!(ks.is_blocked(&peer_fp));
    assert!(!ks.is_trusted(&peer_fp), "blocking must revoke any trust");

    // Reload from disk: all state must be durable.
    drop(ks);
    let ks2 = KeyStore::new(Some(storage.clone())).expect("keystore reload");
    assert!(ks2.is_blocked(&peer_fp), "blocked state must persist");
    assert!(
        ks2.get_all_known_peers()
            .iter()
            .any(|p| p.fingerprint == peer_fp),
        "peer record must persist"
    );

    let _ = std::fs::remove_dir_all(&storage);
}
