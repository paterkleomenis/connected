use connected_core::{ConnectedClient, ConnectedEvent, DeviceType};
use std::net::IpAddr;
use std::path::Path;
use std::time::Duration;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

// Helper to create a dummy file with random bytes of a given size
async fn create_dummy_file(path: &Path, size: usize) -> std::io::Result<()> {
    let mut file = File::create(path).await?;
    let data = vec![0xABu8; size]; // Dummy byte pattern
    file.write_all(&data).await?;
    file.sync_all().await?;
    Ok(())
}

// Helper to wait for a specific event on the client's event receiver
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
async fn test_file_transfer_resume_folders_batches() {
    let root_path = std::env::current_dir()
        .unwrap()
        .join("core/tests/tmp_file_transfer_test");
    let _ = tokio::fs::remove_dir_all(&root_path).await;
    tokio::fs::create_dir_all(&root_path).await.unwrap();

    // Create Sender directories and files
    let send_dir = root_path.join("sender");
    tokio::fs::create_dir_all(&send_dir).await.unwrap();

    let single_file_path = send_dir.join("single_file.bin");
    create_dummy_file(&single_file_path, 6 * 1024 * 1024)
        .await
        .unwrap(); // 6MB file

    let batch_file_1 = send_dir.join("batch_file_1.bin");
    create_dummy_file(&batch_file_1, 1024 * 1024).await.unwrap(); // 1MB
    let batch_file_2 = send_dir.join("batch_file_2.bin");
    create_dummy_file(&batch_file_2, 512 * 1024).await.unwrap(); // 512KB

    let folder_to_send = send_dir.join("my_folder");
    let nested_dir = folder_to_send.join("sub");
    tokio::fs::create_dir_all(&nested_dir).await.unwrap();
    let folder_file_1 = folder_to_send.join("file1.bin");
    create_dummy_file(&folder_file_1, 100 * 1024).await.unwrap();
    let folder_file_2 = nested_dir.join("file2.bin");
    create_dummy_file(&folder_file_2, 200 * 1024).await.unwrap();

    // Create Receiver directories
    let recv_dir = root_path.join("receiver");
    tokio::fs::create_dir_all(&recv_dir).await.unwrap();

    // Initialize Client A (Sender) and Client B (Receiver) bound to 127.0.0.1
    let client_a = ConnectedClient::new_with_ip(
        "Sender-Device".to_string(),
        DeviceType::Linux,
        46001,
        IpAddr::from([127, 0, 0, 1]),
        None,
    )
    .await
    .unwrap();

    let client_b = ConnectedClient::new_with_ip(
        "Receiver-Device".to_string(),
        DeviceType::Linux,
        46002,
        IpAddr::from([127, 0, 0, 1]),
        Some(recv_dir.clone()),
    )
    .await
    .unwrap();

    let _rx_a = client_a.subscribe();
    let mut rx_b = client_b.subscribe();

    // Enable pairing mode
    client_a.set_pairing_mode(true);
    client_b.set_pairing_mode(true);

    let ip_a = IpAddr::from([127, 0, 0, 1]);
    let ip_b = IpAddr::from([127, 0, 0, 1]);

    // Step 1: Initiate Handshake from A to B in the background
    let client_a_clone = client_a.clone();
    let handshake_handle =
        tokio::spawn(async move { client_a_clone.send_handshake(ip_b, 46002).await });

    // Step 2: B receives PairingRequest and trusts A
    let pairing_req = wait_for_event(&mut rx_b, Duration::from_secs(5), |event| {
        matches!(event, ConnectedEvent::PairingRequest { .. })
    })
    .await
    .unwrap();

    if let ConnectedEvent::PairingRequest {
        fingerprint,
        device_id,
        device_name,
    } = pairing_req
    {
        client_b
            .trust_device(fingerprint.clone(), Some(device_id), device_name)
            .unwrap();
        client_b.send_trust_confirmation(ip_a, 46001).await.unwrap();
    } else {
        panic!("Expected PairingRequest");
    }

    // Await handshake completion (A receives HandshakeAck, auto-trusts B, and resolves the future)
    handshake_handle.await.unwrap().unwrap();

    // Verify both sides trust each other
    assert!(
        client_a.is_device_trusted(&client_b.local_device().id),
        "Client A should trust Client B"
    );
    assert!(
        client_b.is_device_trusted(&client_a.local_device().id),
        "Client B should trust Client A"
    );

    println!("Pairing established successfully between Sender and Receiver!");

    // --- TEST CASE 1: Single File Resume ---
    println!("Testing Single File Resume...");

    // Pre-populate a partial .part file on the receiver side with the first 3MB of the file
    let download_dir = client_b.get_download_dir();
    let expected_dest_path = download_dir.join("single_file.bin");
    let part_path = download_dir.join("single_file.bin.part");

    {
        use tokio::io::AsyncReadExt;
        let mut src = File::open(&single_file_path).await.unwrap();
        let mut dst = File::create(&part_path).await.unwrap();
        let mut buf = vec![0u8; 1024 * 1024]; // 1MB buffer
        for _ in 0..3 {
            src.read_exact(&mut buf).await.unwrap();
            dst.write_all(&buf).await.unwrap();
        }
        dst.sync_all().await.unwrap();
    }

    // Start sending the file. It should automatically resume from the 3MB offset.
    let _transfer_id = client_a
        .send_file(ip_b, 46002, single_file_path.clone())
        .await
        .unwrap();

    // Wait for the transfer to complete successfully
    let completed_event = wait_for_event(&mut rx_b, Duration::from_secs(10), |event| {
        matches!(event, ConnectedEvent::TransferCompleted { .. })
    })
    .await
    .unwrap();

    if let ConnectedEvent::TransferCompleted { filename, .. } = completed_event {
        assert_eq!(filename, "single_file.bin");
    } else {
        panic!("Expected TransferCompleted");
    }

    assert!(
        expected_dest_path.exists(),
        "Final file should exist after successful completion"
    );
    assert!(
        !part_path.exists(),
        "Temporary .part file should be cleaned up"
    );

    let final_size = tokio::fs::metadata(&expected_dest_path)
        .await
        .unwrap()
        .len();
    assert_eq!(final_size, 6 * 1024 * 1024, "Final file size mismatch");
    println!("File transfer resumed from 3MB and completed successfully!");

    // --- TEST CASE 2: Native Folder Transfer (No Zipping) ---
    println!("Testing Native Folder Transfer...");

    let _folder_transfer_id = client_a
        .send_file(ip_b, 46002, folder_to_send.clone())
        .await
        .unwrap();

    // Wait for folder transfer completion
    wait_for_event(&mut rx_b, Duration::from_secs(10), |event| {
        matches!(event, ConnectedEvent::TransferCompleted { .. })
    })
    .await
    .unwrap();

    // Verify folder structure is natively recreated
    let dest_folder = download_dir.join("my_folder");
    let dest_nested = dest_folder.join("sub");
    let dest_file_1 = dest_folder.join("file1.bin");
    let dest_file_2 = dest_nested.join("file2.bin");

    assert!(dest_folder.is_dir(), "Destination directory should exist");
    assert!(
        dest_nested.is_dir(),
        "Destination sub-directory should exist"
    );
    assert!(dest_file_1.is_file(), "Destination file1 should exist");
    assert!(dest_file_2.is_file(), "Destination file2 should exist");

    assert_eq!(
        tokio::fs::metadata(&dest_file_1).await.unwrap().len(),
        100 * 1024
    );
    assert_eq!(
        tokio::fs::metadata(&dest_file_2).await.unwrap().len(),
        200 * 1024
    );
    println!("Native folder transfer verified successfully!");

    // --- TEST CASE 3: Batch File Transfer ---
    println!("Testing Batch File Transfer...");

    let batch_files = vec![batch_file_1.clone(), batch_file_2.clone()];
    let _batch_transfer_id = client_a
        .send_files(ip_b, 46002, batch_files.clone())
        .await
        .unwrap();

    // Wait for batch completion
    wait_for_event(&mut rx_b, Duration::from_secs(10), |event| {
        matches!(event, ConnectedEvent::TransferCompleted { .. })
    })
    .await
    .unwrap();

    // Verify batch files arrived in receiver downloads
    let dest_batch_1 = download_dir.join("batch_file_1.bin");
    let dest_batch_2 = download_dir.join("batch_file_2.bin");

    assert!(dest_batch_1.is_file());
    assert!(dest_batch_2.is_file());
    assert_eq!(
        tokio::fs::metadata(&dest_batch_1).await.unwrap().len(),
        1024 * 1024
    );
    assert_eq!(
        tokio::fs::metadata(&dest_batch_2).await.unwrap().len(),
        512 * 1024
    );
    println!("Batch file transfer verified successfully!");

    // --- TEST CASE 4: Batch File Transfer Resume ---
    println!("Testing Batch File Transfer Resume...");

    // Remove the completed files
    tokio::fs::remove_file(&dest_batch_1).await.unwrap();
    tokio::fs::remove_file(&dest_batch_2).await.unwrap();

    // Pre-populate partial .part files for the batch files
    let part_batch_1 = download_dir.join("batch_file_1.bin.part");
    let part_batch_2 = download_dir.join("batch_file_2.bin.part");

    // Write partial content (400KB and 200KB of 0xAB bytes respectively)
    {
        let mut f1 = File::create(&part_batch_1).await.unwrap();
        f1.write_all(&vec![0xABu8; 400 * 1024]).await.unwrap();
        f1.sync_all().await.unwrap();

        let mut f2 = File::create(&part_batch_2).await.unwrap();
        f2.write_all(&vec![0xABu8; 200 * 1024]).await.unwrap();
        f2.sync_all().await.unwrap();
    }

    // Start sending files again
    let _batch_resume_id = client_a.send_files(ip_b, 46002, batch_files).await.unwrap();

    // Wait for batch completion
    wait_for_event(&mut rx_b, Duration::from_secs(10), |event| {
        matches!(event, ConnectedEvent::TransferCompleted { .. })
    })
    .await
    .unwrap();

    // Verify both files completed and are of correct final sizes
    assert!(dest_batch_1.is_file());
    assert!(dest_batch_2.is_file());
    assert_eq!(
        tokio::fs::metadata(&dest_batch_1).await.unwrap().len(),
        1024 * 1024
    );
    assert_eq!(
        tokio::fs::metadata(&dest_batch_2).await.unwrap().len(),
        512 * 1024
    );

    // Verify .part files are removed
    assert!(!part_batch_1.exists());
    assert!(!part_batch_2.exists());
    println!("Batch file transfer resumed from partial files and verified successfully!");

    // Cleanup directories
    let _ = tokio::fs::remove_dir_all(&root_path).await;

    // Shutdown both clients
    client_a.shutdown().await;
    client_b.shutdown().await;
}

#[test]
fn test_path_traversal_safety() {
    // Test that traversal components are caught
    assert!(!connected_core::file_transfer::is_safe_relative_path(
        "../etc/shadow"
    ));
    assert!(!connected_core::file_transfer::is_safe_relative_path(
        "sub/../../private"
    ));
    assert!(!connected_core::file_transfer::is_safe_relative_path(
        "/absolute/path"
    ));

    // Windows paths
    assert!(!connected_core::file_transfer::is_safe_relative_path(
        "C:\\windows"
    ));
    assert!(!connected_core::file_transfer::is_safe_relative_path(
        "sub\\..\\..\\private"
    ));

    // Safe paths
    assert!(connected_core::file_transfer::is_safe_relative_path(
        "my_folder/sub/file.bin"
    ));
    assert!(connected_core::file_transfer::is_safe_relative_path(
        "file.txt"
    ));
    assert!(connected_core::file_transfer::is_safe_relative_path(
        "sub/file"
    ));
}
