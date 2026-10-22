//! End-to-end regression test for the dedicated remote-input stream.
//!
//! A phone and desktop still use the normal pairing handshake; after mutual
//! trust the phone opens stream type 4, performs the input handshake, and the
//! desktop receives ordered batches without touching the control-message path.

use connected_core::{ConnectedClient, ConnectedEvent, DeviceType, RemoteInputEvent};
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

const IP: IpAddr = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
const PHONE_PORT: u16 = 46411;
const DESKTOP_PORT: u16 = 46412;
const SECOND_PHONE_PORT: u16 = 46413;

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

#[tokio::test]
async fn remote_input_stream_round_trip_after_pairing() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("tmp_remote_input_test");
    let _ = tokio::fs::remove_dir_all(&root).await;
    tokio::fs::create_dir_all(&root).await.unwrap();

    let phone = ConnectedClient::new_with_ip(
        "Phone".to_string(),
        DeviceType::Android,
        PHONE_PORT,
        IP,
        Some(root.join("phone")),
    )
    .await
    .unwrap();
    let desktop = ConnectedClient::new_with_ip(
        "Desktop".to_string(),
        DeviceType::Linux,
        DESKTOP_PORT,
        IP,
        Some(root.join("desktop")),
    )
    .await
    .unwrap();

    let mut desktop_events = desktop.subscribe();

    // --- Pair phone <-> desktop -----------------------------------------
    let phone_clone = phone.clone();
    let handshake = tokio::spawn(async move { phone_clone.send_handshake(IP, DESKTOP_PORT).await });

    let request = wait_for_event(&mut desktop_events, Duration::from_secs(10), |event| {
        matches!(event, ConnectedEvent::PairingRequest { .. })
    })
    .await
    .expect("desktop should receive phone's pairing request");

    if let ConnectedEvent::PairingRequest {
        fingerprint,
        device_id,
        device_name,
    } = request
    {
        desktop
            .trust_device(fingerprint, Some(device_id), device_name)
            .unwrap();
        desktop
            .send_trust_confirmation(IP, PHONE_PORT)
            .await
            .unwrap();
    } else {
        unreachable!();
    }
    handshake.await.unwrap().unwrap();

    let desktop_id = desktop.local_device().id.clone();
    assert!(phone.is_device_trusted(&desktop_id));

    // --- Remote input ----------------------------------------------------
    // Start while the desktop gate is off. The handshake is denied, but the
    // sender retries in the background so enabling the gate recovers without
    // waiting for another touch.
    phone.start_remote_input(IP, DESKTOP_PORT).unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(!phone.is_remote_input_active(IP, DESKTOP_PORT));
    desktop.set_remote_input_enabled(true);

    let started = wait_for_event(&mut desktop_events, Duration::from_secs(10), |event| {
        matches!(event, ConnectedEvent::RemoteInputSessionStarted { .. })
    })
    .await
    .expect("desktop should confirm the input handshake after enabling the gate");
    let session_id = match started {
        ConnectedEvent::RemoteInputSessionStarted { session_id, .. } => session_id,
        _ => unreachable!(),
    };

    let expected = vec![
        RemoteInputEvent::PointerMotion { dx: 12.5, dy: -3.0 },
        RemoteInputEvent::PointerButton {
            button: connected_core::input::BTN_LEFT,
            pressed: true,
        },
        RemoteInputEvent::PointerButton {
            button: connected_core::input::BTN_LEFT,
            pressed: false,
        },
        RemoteInputEvent::Text {
            text: "hello".to_string(),
        },
    ];
    phone
        .send_remote_input(IP, DESKTOP_PORT, expected.clone())
        .unwrap();

    let received = wait_for_event(&mut desktop_events, Duration::from_secs(10), |event| {
        matches!(event, ConnectedEvent::RemoteInput { session_id: id, events, .. }
        if *id == session_id
            && events.iter().any(|event| matches!(
                event,
                RemoteInputEvent::PointerButton { pressed: false, .. }
            )))
    })
    .await
    .expect("desktop should receive the input batch");

    match received {
        ConnectedEvent::RemoteInput {
            session_id: id,
            events,
            ..
        } => {
            assert_eq!(id, session_id);
            assert_eq!(events, expected, "batches must remain ordered and intact");
        }
        _ => unreachable!(),
    }

    // The phone-side status flips only after the stream handshake completes.
    for _ in 0..40 {
        if phone.is_remote_input_active(IP, DESKTOP_PORT) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(phone.is_remote_input_active(IP, DESKTOP_PORT));

    // A batch larger than the per-frame limit must be split without dropping
    // ordering. The worker defers the tail rather than coalescing past 256
    // events (which the receiver would reject as a protocol violation).
    let many: Vec<RemoteInputEvent> = (0..300)
        .map(|index| RemoteInputEvent::PointerMotion {
            dx: index as f32,
            dy: -1.0,
        })
        .collect();
    phone
        .send_remote_input(IP, DESKTOP_PORT, many[..150].to_vec())
        .unwrap();
    phone
        .send_remote_input(IP, DESKTOP_PORT, many[150..].to_vec())
        .unwrap();

    let mut received_many = Vec::new();
    while received_many.len() < many.len() {
        let event = wait_for_event(&mut desktop_events, Duration::from_secs(10), |event| {
            matches!(
                event,
                ConnectedEvent::RemoteInput { session_id: id, .. } if *id == session_id
            )
        })
        .await
        .expect("desktop should receive all split batches");
        match event {
            ConnectedEvent::RemoteInput { events, .. } => received_many.extend(events),
            _ => unreachable!(),
        }
    }
    assert_eq!(
        received_many, many,
        "split batches must preserve every event"
    );

    // A second paired phone must not steal the machine while the first
    // session is active. It is denied with AlreadyActive and keeps retrying,
    // then takes over once the first phone disconnects.
    let second_phone = ConnectedClient::new_with_ip(
        "Second Phone".to_string(),
        DeviceType::Android,
        SECOND_PHONE_PORT,
        IP,
        Some(root.join("second-phone")),
    )
    .await
    .unwrap();
    let second_phone_clone = second_phone.clone();
    let second_handshake =
        tokio::spawn(async move { second_phone_clone.send_handshake(IP, DESKTOP_PORT).await });
    let second_request = wait_for_event(&mut desktop_events, Duration::from_secs(10), |event| {
        matches!(event, ConnectedEvent::PairingRequest { .. })
    })
    .await
    .expect("desktop should receive the second phone's pairing request");
    if let ConnectedEvent::PairingRequest {
        fingerprint,
        device_id,
        device_name,
    } = second_request
    {
        desktop
            .trust_device(fingerprint, Some(device_id), device_name)
            .unwrap();
        desktop
            .send_trust_confirmation(IP, SECOND_PHONE_PORT)
            .await
            .unwrap();
    } else {
        unreachable!();
    }
    second_handshake.await.unwrap().unwrap();
    assert!(second_phone.is_device_trusted(&desktop_id));

    let mut second_events = desktop.subscribe();
    second_phone.start_remote_input(IP, DESKTOP_PORT).unwrap();
    let stolen = wait_for_event(&mut second_events, Duration::from_secs(1), |event| {
        matches!(
            event,
            ConnectedEvent::RemoteInputSessionStarted { from_device, .. }
                if from_device == "Second Phone"
        )
    })
    .await;
    assert!(
        stolen.is_none(),
        "a second phone must not replace an active remote-input session"
    );
    assert!(
        phone.is_remote_input_active(IP, DESKTOP_PORT),
        "the first phone must keep its active session"
    );

    // Stop closes the stream. The desktop must report the session end with
    // the same id so a replaced session cannot close a newer one.
    phone.stop_remote_input(IP, DESKTOP_PORT);
    assert!(!phone.is_remote_input_active(IP, DESKTOP_PORT));
    let ended = wait_for_event(&mut desktop_events, Duration::from_secs(10), |event| {
        matches!(
            event,
            ConnectedEvent::RemoteInputSessionEnded { session_id: id, .. } if *id == session_id
        )
    })
    .await;
    assert!(ended.is_some(), "desktop should observe session end");

    // The second phone retries in the background and takes the now-free
    // slot without another user action.
    let second_started = wait_for_event(&mut second_events, Duration::from_secs(10), |event| {
        matches!(
            event,
            ConnectedEvent::RemoteInputSessionStarted { from_device, .. }
                if from_device == "Second Phone"
        )
    })
    .await
    .expect("second phone should start after the first session ends");
    let second_session_id = match second_started {
        ConnectedEvent::RemoteInputSessionStarted { session_id, .. } => session_id,
        _ => unreachable!(),
    };
    for _ in 0..80 {
        if second_phone.is_remote_input_active(IP, DESKTOP_PORT) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(second_phone.is_remote_input_active(IP, DESKTOP_PORT));

    let second_batch = vec![RemoteInputEvent::PointerMotion { dx: 1.0, dy: 2.0 }];
    second_phone
        .send_remote_input(IP, DESKTOP_PORT, second_batch.clone())
        .unwrap();
    let second_received = wait_for_event(&mut second_events, Duration::from_secs(10), |event| {
        matches!(
            event,
            ConnectedEvent::RemoteInput { session_id, events, .. }
                if *session_id == second_session_id && *events == second_batch
        )
    })
    .await;
    assert!(
        second_received.is_some(),
        "the second phone should inject after taking over"
    );

    second_phone.shutdown().await;
    phone.shutdown().await;
    desktop.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}
