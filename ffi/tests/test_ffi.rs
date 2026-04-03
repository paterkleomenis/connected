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
