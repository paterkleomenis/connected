use crate::state::*;
use crate::utils::set_system_clipboard;
use connected_core::facade::{
    ClipboardCallback, DiscoveredDevice, DiscoveryCallback, FileTransferCallback,
};
use std::thread;
use std::time::Duration;
use tracing::{error, info, warn};

pub struct AppDiscoveryCallback;

impl DiscoveryCallback for AppDiscoveryCallback {
    fn on_device_found(&self, device: DiscoveredDevice) {
        let info: DeviceInfo = device.into();
        info!("Device found: {} ({}:{})", info.name, info.ip, info.port);

        let mut store = get_devices_store().lock().unwrap();

        // Remove any existing device with same IP (device restarted with new ID)
        let old_ids: Vec<String> = store
            .iter()
            .filter(|(_, d)| d.ip == info.ip && d.id != info.id)
            .map(|(id, _)| id.clone())
            .collect();
        for old_id in old_ids {
            store.remove(&old_id);
        }

        // Only notify if this is a truly new device (not an update)
        if !store.contains_key(&info.id) {
            add_notification(
                "Device Found",
                &format!("{} is now available", info.name),
                "📱",
            );
        }

        store.insert(info.id.clone(), info);
    }

    fn on_device_lost(&self, device_id: String) {
        info!("Device lost: {}", device_id);
        if let Some(device) = get_devices_store().lock().unwrap().remove(&device_id) {
            add_notification(
                "Device Lost",
                &format!("{} disconnected", device.name),
                "📴",
            );
        }
    }

    fn on_error(&self, error_msg: String) {
        error!("Discovery error: {}", error_msg);
        add_notification("Discovery Error", &error_msg, "⚠️");
    }
}

pub struct AppFileTransferCallback;

impl FileTransferCallback for AppFileTransferCallback {
    fn on_transfer_starting(&self, filename: String, total_size: u64) {
        info!("Transfer starting: {} ({} bytes)", filename, total_size);
        *get_transfer_status().lock().unwrap() = TransferStatus::Starting(filename.clone());
        add_notification("Transfer Started", &format!("Receiving {}", filename), "📥");
    }

    fn on_transfer_progress(&self, bytes_transferred: u64, total_size: u64) {
        let percent = if total_size > 0 {
            (bytes_transferred as f32 / total_size as f32) * 100.0
        } else {
            0.0
        };

        let mut status = get_transfer_status().lock().unwrap();
        if let TransferStatus::Starting(ref filename)
        | TransferStatus::InProgress { ref filename, .. } = *status
        {
            let filename = filename.clone();
            *status = TransferStatus::InProgress { filename, percent };
        }
    }

    fn on_transfer_completed(&self, filename: String, total_size: u64) {
        info!("Transfer completed: {} ({} bytes)", filename, total_size);
        *get_transfer_status().lock().unwrap() = TransferStatus::Completed(filename.clone());
        add_notification(
            "Transfer Complete",
            &format!(
                "{} received ({:.1} MB)",
                filename,
                total_size as f64 / 1_000_000.0
            ),
            "✅",
        );

        // Reset after a delay
        thread::spawn(|| {
            thread::sleep(Duration::from_secs(3));
            *get_transfer_status().lock().unwrap() = TransferStatus::Idle;
        });
    }

    fn on_transfer_failed(&self, error_msg: String) {
        error!("Transfer failed: {}", error_msg);
        *get_transfer_status().lock().unwrap() = TransferStatus::Failed(error_msg.clone());
        add_notification("Transfer Failed", &error_msg, "❌");

        thread::spawn(|| {
            thread::sleep(Duration::from_secs(5));
            *get_transfer_status().lock().unwrap() = TransferStatus::Idle;
        });
    }

    fn on_transfer_cancelled(&self) {
        warn!("Transfer cancelled");
        *get_transfer_status().lock().unwrap() = TransferStatus::Idle;
        add_notification(
            "Transfer Cancelled",
            "The file transfer was cancelled",
            "🚫",
        );
    }
}

pub struct AppClipboardCallback;

impl ClipboardCallback for AppClipboardCallback {
    fn on_clipboard_received(&self, text: String, from_device: String) {
        info!(
            "Clipboard received from {}: {} chars",
            from_device,
            text.len()
        );
        *get_clipboard_status().lock().unwrap() = ClipboardStatus::Received {
            from: from_device.clone(),
            text: text.clone(),
        };

        set_system_clipboard(&text);

        *get_last_clipboard().lock().unwrap() = text.clone();

        let preview = if text.len() > 50 {
            format!("{}...", &text[..50])
        } else {
            text
        };
        add_notification(
            "Clipboard Received",
            &format!("From {}: {}", from_device, preview),
            "📋",
        );
    }

    fn on_clipboard_sent(&self, success: bool, error_msg: Option<String>) {
        if success {
            info!("Clipboard sent successfully");
            *get_clipboard_status().lock().unwrap() = ClipboardStatus::Sent { success: true };
        } else {
            let msg = error_msg.unwrap_or_else(|| "Unknown error".to_string());
            error!("Clipboard send failed: {}", msg);
            add_notification("Clipboard Failed", &msg, "⚠️");
        }
    }
}
