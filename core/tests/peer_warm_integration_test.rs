use connected_core::{ConnectedClient, ConnectedEvent, DeviceType};
use std::net::IpAddr;
use std::time::Duration;

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

async fn fresh_storage(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "connected-test-warm-{}-{}",
        tag,
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&dir).expect("create test storage dir");
    dir
}

async fn poll_until(timeout_duration: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout_duration {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    cond()
}

/// Pair two clients with the real user-initiated handshake flow, then verify
/// mutual trust. Mirrors production pairing; the warmer tests below build on it.
async fn pair(
    a: &std::sync::Arc<ConnectedClient>,
    b: &std::sync::Arc<ConnectedClient>,
    rx_b: &mut tokio::sync::broadcast::Receiver<ConnectedEvent>,
    ip: IpAddr,
    port_a: u16,
    port_b: u16,
) {
    a.set_pairing_mode(true);
    b.set_pairing_mode(true);

    let a_clone = a.clone();
    let handshake_handle = tokio::spawn(async move { a_clone.send_handshake(ip, port_b).await });

    let pairing_req = wait_for_event(rx_b, Duration::from_secs(10), |event| {
        matches!(event, ConnectedEvent::PairingRequest { .. })
    })
    .await
    .expect("B should receive a pairing request");

    if let ConnectedEvent::PairingRequest {
        fingerprint,
        device_id,
        device_name,
    } = pairing_req
    {
        b.trust_device(fingerprint.clone(), Some(device_id), device_name)
            .unwrap();
        b.send_trust_confirmation(ip, port_a).await.unwrap();
    } else {
        panic!("Expected PairingRequest");
    }

    handshake_handle.await.unwrap().unwrap();

    assert!(
        a.is_device_trusted(&b.local_device().id),
        "A should trust B after pairing"
    );
    assert!(
        b.is_device_trusted(&a.local_device().id),
        "B should trust A after pairing"
    );
}

/// The core firewall-bypass mechanism: after pairing, with all connections
/// wiped (simulating a firewall appearing / an app restart with no live
/// connections), `ensure_peer_connection` must re-establish a path WITHOUT
/// any pairing flow — and the receiving side must register a connection alias
/// so its own future sends reuse the inbound connection instead of dialing.
#[tokio::test]
async fn test_hello_registers_alias_for_connection_reuse() {
    let ip = IpAddr::from([127, 0, 0, 1]);
    let (port_a, port_b) = (47101u16, 47102u16);

    // Isolated keystores: sharing the default config dir would alias every
    // client to the developer's real identity/trust set and let parallel
    // tests (or the real LAN) cross-talk into these assertions.
    let a = ConnectedClient::new_with_ip(
        "Warm-A".to_string(),
        DeviceType::Linux,
        port_a,
        IpAddr::from([127, 0, 0, 1]),
        Some(fresh_storage("a").await),
    )
    .await
    .unwrap();
    let b = ConnectedClient::new_with_ip(
        "Warm-B".to_string(),
        DeviceType::Linux,
        port_b,
        IpAddr::from([127, 0, 0, 1]),
        Some(fresh_storage("b").await),
    )
    .await
    .unwrap();

    let mut rx_b = b.subscribe();
    pair(&a, &b, &mut rx_b, ip, port_a, port_b).await;

    // Wipe every connection on both sides: the dialer-side entry, the
    // acceptor-side ephemeral entry, and any pairing-time alias. Closing via
    // any key kills the shared connection object; closed entries prune lazily.
    a.invalidate_connection(ip, port_b);
    b.invalidate_connection(ip, port_a);
    assert!(
        !a.has_live_connection(ip, port_b),
        "A should have no live path to B after invalidation"
    );
    assert!(
        !b.has_live_connection(ip, port_a),
        "B should have no live path to A after invalidation"
    );

    // Warm the path. No pairing state is touched: this must work purely off
    // existing trust.
    let b_id = b.local_device().id.clone();
    a.ensure_peer_connection(&b_id, ip, port_b).await;

    // Dialer side: direct cache entry.
    assert!(
        poll_until(Duration::from_secs(5), || a.has_live_connection(ip, port_b)).await,
        "A should have a live connection to B after warming"
    );
    // Acceptor side: alias (B's listen addr was never dialed by B) — this is
    // the assertion that proves Hello + inbound caching + alias all work.
    assert!(
        poll_until(Duration::from_secs(5), || b.has_live_connection(ip, port_a)).await,
        "B should hold an aliased connection for A's listen address after Hello"
    );

    // Warming is idempotent and silent: no pairing UI events on repeat.
    let mut rx_b2 = b.subscribe();
    a.ensure_peer_connection(&b_id, ip, port_b).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let mut pairing_spam = false;
    while let Ok(ev) = rx_b2.try_recv() {
        if matches!(ev, ConnectedEvent::PairingRequest { .. }) {
            pairing_spam = true;
        }
    }
    assert!(
        !pairing_spam,
        "Hello must never emit PairingRequest for trusted peers"
    );

    a.shutdown().await;
    b.shutdown().await;
}

/// Untrusted peers must never be dialed or helloed: no connection appears on
/// their side and no pairing prompt is triggered there.
#[tokio::test]
async fn test_warmer_ignores_untrusted_peers() {
    let ip = IpAddr::from([127, 0, 0, 1]);
    let (port_a, port_c) = (47103u16, 47104u16);

    // Isolated keystores (see above): without these, both clients would load
    // the developer's real identity and trust set.
    let a = ConnectedClient::new_with_ip(
        "Warm-A2".to_string(),
        DeviceType::Linux,
        port_a,
        IpAddr::from([127, 0, 0, 1]),
        Some(fresh_storage("a2").await),
    )
    .await
    .unwrap();
    let c = ConnectedClient::new_with_ip(
        "Stranger-C".to_string(),
        DeviceType::Linux,
        port_c,
        IpAddr::from([127, 0, 0, 1]),
        Some(fresh_storage("c").await),
    )
    .await
    .unwrap();

    let mut rx_c = c.subscribe();
    let c_id = c.local_device().id.clone();
    assert!(
        !a.is_device_trusted(&c_id),
        "precondition: C is not trusted by A"
    );

    a.ensure_peer_connection(&c_id, ip, port_c).await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    assert!(
        !c.has_live_connection(ip, port_a),
        "untrusted C must gain no connection from A's warmer"
    );
    assert!(
        !a.has_live_connection(ip, port_c),
        "A must not dial untrusted peers"
    );

    // Drain anything pairing-related (mDNS cross-talk from parallel tests can
    // legitimately produce other events); fail only on PairingRequest.
    let mut pairing_seen = false;
    while let Ok(ev) = rx_c.try_recv() {
        if matches!(ev, ConnectedEvent::PairingRequest { .. }) {
            pairing_seen = true;
        }
    }
    assert!(
        !pairing_seen,
        "warming an untrusted peer must not raise a pairing prompt"
    );

    a.shutdown().await;
    c.shutdown().await;
}
