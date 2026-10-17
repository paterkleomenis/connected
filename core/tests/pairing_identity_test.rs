//! Regression tests for pairing-state identity binding.
//!
//! Security context: pending pairing requests used to be keyed by the
//! peer-claimed `device_id`. Device ids are advertised via mDNS TXT records,
//! and recording a request silently overwrote any existing entry with the
//! same id — so a LAN impostor claiming the id of the device the user was
//! pairing with could replace its pending request with their own
//! certificate, and the user's approval of the genuine-looking prompt then
//! granted full trust to the impostor. Requests are now keyed by the TLS
//! fingerprint — the identity actually shown in the prompt and approved by
//! `trust_device` — and a claimed id alone can no longer displace anything.

use connected_core::codec::encode_message;
use connected_core::security::KeyStore;
use connected_core::transport::QuicTransport;
use connected_core::{ConnectedClient, ConnectedEvent, DeviceType, Message};
use parking_lot::RwLock;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

const IP: IpAddr = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));

/// Wait for the next `PairingRequest`, returning `(fingerprint, device_id)`.
async fn wait_for_pairing_request(
    rx: &mut tokio::sync::broadcast::Receiver<ConnectedEvent>,
    timeout: Duration,
) -> Option<(String, String)> {
    tokio::time::timeout(timeout, async {
        while let Ok(event) = rx.recv().await {
            if let ConnectedEvent::PairingRequest {
                fingerprint,
                device_id,
                ..
            } = event
            {
                return Some((fingerprint, device_id));
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

#[tokio::test]
async fn forged_device_id_claim_cannot_displace_pending_pairing_request() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("tmp_pairing_identity_test");
    let _ = tokio::fs::remove_dir_all(&root).await;
    tokio::fs::create_dir_all(&root).await.unwrap();

    let client_a = ConnectedClient::new_with_ip(
        "Legit Device".to_string(),
        DeviceType::Linux,
        46301,
        IP,
        None,
    )
    .await
    .unwrap();

    let client_b = ConnectedClient::new_with_ip(
        "Receiver".to_string(),
        DeviceType::Linux,
        46302,
        IP,
        Some(root.join("recv")),
    )
    .await
    .unwrap();

    let mut rx_b = client_b.subscribe();

    let legit_fp = client_a.get_fingerprint();
    let legit_id = client_a.local_device().id.clone();

    // 1. The legitimate device starts pairing with B: B records a pending
    //    request and shows the approval prompt.
    let a_clone = client_a.clone();
    let handshake_handle = tokio::spawn(async move { a_clone.send_handshake(IP, 46302).await });

    let (first_fp, first_id) = wait_for_pairing_request(&mut rx_b, Duration::from_secs(10))
        .await
        .expect("B should show the legitimate pairing request");
    assert_eq!(
        first_fp, legit_fp,
        "first prompt must carry the real device's fingerprint"
    );
    assert_eq!(first_id, legit_id);

    // 2. An impostor connects and sends a Handshake claiming the victim's
    //    device_id (which is public: it travels in mDNS TXT records).
    let attacker_ks = Arc::new(RwLock::new(
        KeyStore::new(Some(root.join("attacker_keystore"))).unwrap(),
    ));
    let attacker_fp = attacker_ks.read().fingerprint();
    let attacker_transport = QuicTransport::new(
        SocketAddr::new(IP, 46309),
        "attacker".to_string(),
        attacker_ks.clone(),
    )
    .await
    .unwrap();

    let (mut send, _recv) = attacker_transport
        .open_stream_allow_unknown(
            SocketAddr::new(IP, 46302),
            QuicTransport::STREAM_TYPE_CONTROL,
        )
        .await
        .expect("impostor should be able to dial B");

    let forged = Message::Handshake {
        device_id: legit_id.clone(),
        device_name: "Legit Device".to_string(),
        listening_port: 46309,
        protocol_version: connected_core::PROTOCOL_VERSION,
    };
    let data = encode_message(&forged, connected_core::PROTOCOL_VERSION).unwrap();
    send.write_all(&(data.len() as u32).to_be_bytes())
        .await
        .unwrap();
    send.write_all(&data).await.unwrap();
    send.finish().unwrap();

    let (second_fp, second_id) = wait_for_pairing_request(&mut rx_b, Duration::from_secs(10))
        .await
        .expect("B should also show the impostor's request (its own fingerprint)");
    assert_eq!(
        second_fp, attacker_fp,
        "impostor request must be keyed by its own TLS fingerprint"
    );
    assert_eq!(
        second_id, legit_id,
        "impostor claimed the victim's device id"
    );
    assert_ne!(second_fp, legit_fp);

    // 3. Approving the genuine prompt must still succeed even though an
    //    impostor claimed the same device_id. Before the fix the impostor's
    //    request had replaced the genuine one, and this approval failed with
    //    "Pairing request is no longer active" — leaving the user with a
    //    prompt that, if approved, trusted the impostor instead.
    client_b
        .trust_device(
            legit_fp.clone(),
            Some(legit_id.clone()),
            "Legit Device".to_string(),
        )
        .expect("approving the genuine device must succeed");

    let trusted = client_b.get_trusted_peers();
    assert!(
        trusted.iter().any(|p| p.fingerprint == legit_fp),
        "genuine device must be trusted"
    );
    assert!(
        !trusted.iter().any(|p| p.fingerprint == attacker_fp),
        "impostor must NOT be trusted"
    );

    // 4. Complete the exchange so the legitimate initiator's future resolves.
    client_b.send_trust_confirmation(IP, 46301).await.unwrap();
    handshake_handle.await.unwrap().unwrap();

    assert!(
        client_a.is_device_trusted(&client_b.local_device().id),
        "legitimate initiator must trust the receiver after the ack"
    );

    let _ = tokio::fs::remove_dir_all(&root).await;
}
