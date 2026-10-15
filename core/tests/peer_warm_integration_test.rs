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

/// A discovered but not-yet-paired peer is warmed too — that is what lets a
/// *first* pairing initiated from a firewalled side succeed: the warm dial
/// travels outbound, the peer registers the reverse path (Hello alias), and
/// the pairing handshake later rides back over that connection instead of
/// dialing into the firewall.
///
/// Warming must stay trust-wise inert: no pairing prompts, no trust changes,
/// and strict sends must NOT reuse the untrusted connection (that would skip
/// certificate verification — pinned here via a strict ping, which must fail
/// with the peer unknown).
#[tokio::test]
async fn test_untrusted_warm_registers_reverse_path_without_pairing() {
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

    // Dialer side: an untrusted discovered peer gets a warm dial now.
    assert!(
        poll_until(Duration::from_secs(5), || a.has_live_connection(ip, port_c)).await,
        "A should dial untrusted discovered peers when warming"
    );
    // Acceptor side: the Hello registers A's listen address as an alias on C
    // (this is the reverse path a first pairing can ride on; the trusted-peer
    // test above pins the same alias mechanism).
    assert!(
        poll_until(Duration::from_secs(5), || c.has_live_connection(ip, port_a)).await,
        "C should hold a reverse-path alias for A's listen address after Hello"
    );

    // Trust-wise inert: nobody got trusted, and no pairing prompt appeared.
    assert!(!a.is_device_trusted(&c_id), "warming must not trust anyone");
    assert!(
        !c.is_device_trusted(&a.local_device().id),
        "warming must not trust anyone"
    );
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

    // Strict sends must not reuse the warmed untrusted connection: the strict
    // `connect` path re-verifies the peer certificate (unknown here) and dials
    // fresh instead, so this ping must fail. Without the guard it would ride
    // the cached connection and succeed — i.e. cert checks would be skipped.
    let ping = c.send_ping(ip, port_a).await;
    assert!(
        ping.is_err(),
        "strict sends must not reuse an untrusted warmed connection (got RTT {}ms)",
        ping.unwrap_or_default()
    );

    a.shutdown().await;
    c.shutdown().await;
}

/// First pairing over a warmed reverse path — the exact scenario the warmer
/// exists for. A (desktop behind a default-deny firewall) warms untrusted C;
/// C initiates pairing and never needs A to be reachable inbound. No pairing
/// mode is enabled on either side: the flow is purely user-initiated.
#[tokio::test]
async fn test_first_pairing_over_warmed_reverse_path() {
    let ip = IpAddr::from([127, 0, 0, 1]);
    let (port_a, port_c) = (47105u16, 47106u16);

    let a = ConnectedClient::new_with_ip(
        "Pair-A3".to_string(),
        DeviceType::Linux,
        port_a,
        IpAddr::from([127, 0, 0, 1]),
        Some(fresh_storage("a3").await),
    )
    .await
    .unwrap();
    let c = ConnectedClient::new_with_ip(
        "Pair-C3".to_string(),
        DeviceType::Linux,
        port_c,
        IpAddr::from([127, 0, 0, 1]),
        Some(fresh_storage("c3").await),
    )
    .await
    .unwrap();

    let mut rx_a = a.subscribe();
    let c_id = c.local_device().id.clone();
    let a_id = a.local_device().id.clone();

    // A warms untrusted C. This is the connection C's pairing will reuse.
    a.ensure_peer_connection(&c_id, ip, port_c).await;
    assert!(
        poll_until(Duration::from_secs(5), || c.has_live_connection(ip, port_a)).await,
        "precondition: C holds the warmed path before pairing starts"
    );

    // C starts pairing (user taps "Pair" on the firewalled side).
    let c_clone = c.clone();
    let handshake_handle = tokio::spawn(async move { c_clone.send_handshake(ip, port_a).await });

    let pairing_req = wait_for_event(&mut rx_a, Duration::from_secs(10), |event| {
        matches!(event, ConnectedEvent::PairingRequest { .. })
    })
    .await
    .expect("A should receive the pairing request over the warmed path");

    if let ConnectedEvent::PairingRequest {
        fingerprint,
        device_id,
        device_name,
    } = pairing_req
    {
        assert_eq!(
            device_id, c_id,
            "pairing request must come from C (the initiator)"
        );
        a.trust_device(fingerprint, Some(device_id), device_name)
            .unwrap();
        a.send_trust_confirmation(ip, port_c).await.unwrap();
    } else {
        panic!("Expected PairingRequest");
    }

    handshake_handle.await.unwrap().unwrap();

    assert!(c.is_device_trusted(&a_id), "C should trust A after pairing");
    assert!(a.is_device_trusted(&c_id), "A should trust C after pairing");

    a.shutdown().await;
    c.shutdown().await;
}

/// Regression for the live first-pairing failure: the human on the other side
/// approves *after* the initiator's in-line ack wait (HANDSHAKE_TIMEOUT, 30s)
/// has already expired — common when the approver is away from the machine.
///
/// The in-line deadline used to surface as `PairingFailed("Timeout: ...")`,
/// which (a) skipped the reverse-path wait that keeps the attempt alive and
/// (b) dropped the pending handshake, so the late trust confirmation was
/// silently ignored and pairing failed for good. The attempt must instead
/// survive the in-line timeout and complete when the ack finally arrives.
#[tokio::test]
async fn test_pairing_survives_approval_after_inline_timeout() {
    let ip = IpAddr::from([127, 0, 0, 1]);
    let (port_a, port_c) = (47107u16, 47108u16);

    let a = ConnectedClient::new_with_ip(
        "Slow-A4".to_string(),
        DeviceType::Linux,
        port_a,
        IpAddr::from([127, 0, 0, 1]),
        Some(fresh_storage("a4").await),
    )
    .await
    .unwrap();
    let c = ConnectedClient::new_with_ip(
        "Slow-C4".to_string(),
        DeviceType::Linux,
        port_c,
        IpAddr::from([127, 0, 0, 1]),
        Some(fresh_storage("c4").await),
    )
    .await
    .unwrap();

    let mut rx_a = a.subscribe();
    let c_id = c.local_device().id.clone();
    let a_id = a.local_device().id.clone();

    // Same shape as the live failure: the request is *delivered* instantly over
    // an already-warmed path (no dial problem involved at all).
    a.ensure_peer_connection(&c_id, ip, port_c).await;
    assert!(
        poll_until(Duration::from_secs(5), || c.has_live_connection(ip, port_a)).await,
        "precondition: C holds the warmed path before pairing starts"
    );

    let c_clone = c.clone();
    let started = std::time::Instant::now();
    let handshake_handle = tokio::spawn(async move { c_clone.send_handshake(ip, port_a).await });

    let pairing_req = wait_for_event(&mut rx_a, Duration::from_secs(10), |event| {
        matches!(event, ConnectedEvent::PairingRequest { .. })
    })
    .await
    .expect("A should receive the pairing request over the warmed path");

    // The slow human: approve only after C's in-line wait is long gone.
    tokio::time::sleep(Duration::from_secs(32)).await;

    if let ConnectedEvent::PairingRequest {
        fingerprint,
        device_id,
        device_name,
    } = pairing_req
    {
        assert_eq!(device_id, c_id, "pairing request must come from C");
        a.trust_device(fingerprint, Some(device_id), device_name)
            .unwrap();
        a.send_trust_confirmation(ip, port_c).await.unwrap();
    } else {
        panic!("Expected PairingRequest");
    }

    let result = handshake_handle.await.unwrap();
    assert!(
        result.is_ok(),
        "pairing must survive an approval that lands after the in-line timeout: {:?}",
        result.err()
    );
    assert!(
        started.elapsed() >= Duration::from_secs(31),
        "the handshake must still have been pending past the 30s in-line window \
         (settled after {:?})",
        started.elapsed()
    );

    assert!(c.is_device_trusted(&a_id), "C should trust A after pairing");
    assert!(a.is_device_trusted(&c_id), "A should trust C after pairing");

    a.shutdown().await;
    c.shutdown().await;
}
