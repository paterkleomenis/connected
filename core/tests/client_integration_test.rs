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

    client.shutdown().await;
}
