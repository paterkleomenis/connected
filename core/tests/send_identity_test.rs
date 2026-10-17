//! Regression tests for outbound file-send identity binding.
//!
//! Security context: `send_file` / `send_files` used to dial every target with
//! the allow-unknown client config and never check which identity answered.
//! mDNS endpoints are unauthenticated (spoofable, stale, or recycled by DHCP),
//! so a file could be streamed to an impostor squatting on the address the UI
//! had learned for a *trusted* device. Sends now bind to the identity the
//! endpoint is known to host: strict certificate verification plus an exact
//! fingerprint check, with one fresh redial to survive stale cache entries —
//! and `IdentityMismatch` otherwise.

use connected_core::codec::encode_message;
use connected_core::security::KeyStore;
use connected_core::transport::QuicTransport;
use connected_core::{ConnectedClient, ConnectedError, ConnectedEvent, DeviceType, Message};
use parking_lot::RwLock;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

const IP: IpAddr = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));

async fn wait_for_event(
    rx: &mut tokio::sync::broadcast::Receiver<ConnectedEvent>,
    timeout_duration: Duration,
    check: impl Fn(&ConnectedEvent) -> bool,
) -> Option<ConnectedEvent> {
    tokio::time::timeout(timeout_duration, async {
        while let Ok(event) = rx.recv().await {
            if check(&event) {
                return Some(event);
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

async fn create_dummy_file(path: &std::path::Path, size: usize) -> std::io::Result<()> {
    let mut file = File::create(path).await?;
    file.write_all(&vec![0xCDu8; size]).await?;
    file.sync_all().await?;
    Ok(())
}

#[tokio::test]
async fn file_send_refuses_endpoint_impersonating_another_trusted_device() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("tmp_send_identity_test");
    let _ = tokio::fs::remove_dir_all(&root).await;
    tokio::fs::create_dir_all(&root).await.unwrap();
    let recv_b = root.join("recv_b");
    tokio::fs::create_dir_all(&recv_b).await.unwrap();

    // A (sender), B (legitimate receiver).
    let client_a = ConnectedClient::new_with_ip(
        "Sender".to_string(),
        DeviceType::Linux,
        46311,
        IP,
        Some(root.join("sender")),
    )
    .await
    .unwrap();
    let client_b = ConnectedClient::new_with_ip(
        "Receiver".to_string(),
        DeviceType::Linux,
        46312,
        IP,
        Some(recv_b.clone()),
    )
    .await
    .unwrap();

    let mut rx_b = client_b.subscribe();
    let mut rx_a = client_a.subscribe();

    // --- Pair A <-> B ------------------------------------------------
    let a_clone = client_a.clone();
    let handshake_handle = tokio::spawn(async move { a_clone.send_handshake(IP, 46312).await });

    let pairing_req = wait_for_event(&mut rx_b, Duration::from_secs(10), |event| {
        matches!(event, ConnectedEvent::PairingRequest { .. })
    })
    .await
    .expect("B should receive A's pairing request");

    if let ConnectedEvent::PairingRequest {
        fingerprint,
        device_id,
        device_name,
    } = pairing_req
    {
        client_b
            .trust_device(fingerprint, Some(device_id), device_name)
            .unwrap();
        client_b.send_trust_confirmation(IP, 46311).await.unwrap();
    } else {
        unreachable!();
    }
    handshake_handle.await.unwrap().unwrap();

    let b_id = client_b.local_device().id.clone();
    assert!(client_a.is_device_trusted(&b_id));

    // --- The impostor: a reachable endpoint that A trusts but that is NOT B
    //     and advertises no endpoint of its own (listening_port 0), so any
    //     discovery mapping to its address can only have come from spoofing.
    let impostor_ks = Arc::new(RwLock::new(
        KeyStore::new(Some(root.join("impostor_keystore"))).unwrap(),
    ));
    let impostor_fp = impostor_ks.read().fingerprint();
    let impostor_transport = QuicTransport::new(
        SocketAddr::new(IP, 46319),
        "impostor".to_string(),
        impostor_ks.clone(),
    )
    .await
    .unwrap();

    // Accept inbound connections so TLS handshakes with the impostor can
    // complete (the send path must reject it on identity, not on reachability).
    let (msg_tx, _msg_rx) = tokio::sync::mpsc::channel(64);
    let (file_tx, _file_rx) = tokio::sync::mpsc::channel(4);
    let (fs_tx, _fs_rx) = tokio::sync::mpsc::channel(4);
    impostor_transport
        .start_server(msg_tx, file_tx, fs_tx)
        .await
        .unwrap();

    // The impostor pairs with A so that A's strict verifier would accept its
    // certificate — the send-side fingerprint check must still reject it.
    let (mut send, _recv) = impostor_transport
        .open_stream_allow_unknown(
            SocketAddr::new(IP, 46311),
            QuicTransport::STREAM_TYPE_CONTROL,
        )
        .await
        .unwrap();
    let forged = Message::Handshake {
        device_id: "impostor-id".to_string(),
        device_name: "Impostor".to_string(),
        listening_port: 0,
        protocol_version: connected_core::PROTOCOL_VERSION,
    };
    let data = encode_message(&forged, connected_core::PROTOCOL_VERSION).unwrap();
    send.write_all(&(data.len() as u32).to_be_bytes())
        .await
        .unwrap();
    send.write_all(&data).await.unwrap();
    send.finish().unwrap();

    let pairing_req = wait_for_event(&mut rx_a, Duration::from_secs(10), |event| {
        matches!(event, ConnectedEvent::PairingRequest { fingerprint, .. } if *fingerprint == impostor_fp)
    })
    .await
    .expect("A should show the impostor's pairing request");
    if let ConnectedEvent::PairingRequest {
        fingerprint,
        device_id,
        device_name,
    } = pairing_req
    {
        client_a
            .trust_device(fingerprint, Some(device_id), device_name)
            .unwrap();
    }

    // --- Positive control: the real endpoint keeps working ------------
    let real_file = root.join("real.txt");
    create_dummy_file(&real_file, 64 * 1024).await.unwrap();
    // Make sure A associates B's id with B's real endpoint.
    client_a
        .inject_proximity_device(
            b_id.clone(),
            "Receiver".to_string(),
            DeviceType::Linux,
            IP,
            46312,
        )
        .unwrap();

    client_a
        .send_file(IP, 46312, real_file)
        .await
        .expect("sending to the genuine endpoint must succeed");
    let completed = wait_for_event(&mut rx_b, Duration::from_secs(15), |event| {
        matches!(event, ConnectedEvent::TransferCompleted { .. })
    })
    .await
    .expect("B should complete the transfer from the genuine sender");
    if let ConnectedEvent::TransferCompleted { filename, .. } = completed {
        assert_eq!(filename, "real.txt");
    }

    // --- Spoofed discovery: B's (public) device_id now points at the
    //     impostor's address. Sending must fail closed, not stream the file.
    client_a
        .inject_proximity_device(
            b_id.clone(),
            "Receiver".to_string(),
            DeviceType::Linux,
            IP,
            46319,
        )
        .unwrap();

    let secret_file = root.join("secret.txt");
    create_dummy_file(&secret_file, 32 * 1024).await.unwrap();

    let err = client_a
        .send_file(IP, 46319, secret_file)
        .await
        .expect_err("sending to an endpoint impersonating a trusted device must fail");
    assert!(
        matches!(err, ConnectedError::IdentityMismatch),
        "expected IdentityMismatch, got: {err}"
    );

    let _ = tokio::fs::remove_dir_all(&root).await;
}
