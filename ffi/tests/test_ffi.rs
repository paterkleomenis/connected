use connected_core::{Device, DeviceType};

#[test]
fn test_core_device_basic() {
    let dev = Device::new(
        "test-id".to_string(),
        "test-name".to_string(),
        "127.0.0.1".parse().unwrap(),
        8080,
        DeviceType::Android,
    );
    assert_eq!(dev.id, "test-id");
    assert_eq!(dev.device_type, DeviceType::Android);
}

/// The FFI layer maps device-type strings coming from Kotlin/Swift through
/// `DeviceType::from_str`; an unmapped string silently degrades to Unknown.
/// Lock the mapping down so a rename on either side of the boundary is
/// caught here instead of surfacing as wrong device icons in the UI.
#[test]
fn test_device_type_string_contract() {
    use std::str::FromStr;

    assert_eq!(DeviceType::from_str("android"), Ok(DeviceType::Android));
    assert_eq!(DeviceType::from_str("ios"), Ok(DeviceType::IOS));
    assert_eq!(DeviceType::from_str("linux"), Ok(DeviceType::Linux));
    assert_eq!(DeviceType::from_str("windows"), Ok(DeviceType::Windows));
    assert_eq!(DeviceType::from_str("macos"), Ok(DeviceType::MacOS));
    // Aliases used by mobile TXT records / share intents:
    assert_eq!(DeviceType::from_str("iphone"), Ok(DeviceType::IOS));

    // Unknown / empty strings must degrade gracefully, never panic — the FFI
    // layer passes peer-supplied TXT-record values straight into this.
    assert_eq!(DeviceType::from_str("toaster"), Err(()));
    assert_eq!(DeviceType::from_str(""), Err(()));

    // Round-trip: every type's storage string parses back to itself.
    for ty in [
        DeviceType::Android,
        DeviceType::IOS,
        DeviceType::Linux,
        DeviceType::Windows,
        DeviceType::MacOS,
        DeviceType::Unknown,
    ] {
        assert_eq!(
            DeviceType::from_str(ty.as_str()),
            Ok(ty),
            "as_str/from_str round-trip broken for {:?}",
            ty
        );
    }
}

/// Contract test for FFI error lifting BEFORE initialize(): exported
/// functions that need the client singleton must return a structured
/// InitializationError (which UniFFI converts into a binding exception) —
/// NOT panic, hang, or return garbage data. This is the first thing a
/// mobile integration hits when startup ordering regresses.
#[test]
fn test_ffi_calls_before_initialize_return_structured_error() {
    // NOTE: these run against an uninitialized process state; they must not
    // panic and must surface the initialization error.
    let devices = connected_ffi::get_discovered_devices();
    assert!(
        devices.is_err(),
        "get_discovered_devices before initialize() must error, not crash"
    );

    // Boolean-style exports degrade to a safe default instead of erroring.
    assert!(
        !connected_ffi::is_device_trusted("any-device".to_string()),
        "nothing can be trusted before initialization"
    );
}
