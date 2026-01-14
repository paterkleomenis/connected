use btleplug::api::{Central, CentralEvent, Manager as _, ScanFilter};
use btleplug::platform::Manager;
use connected_core::{ConnectedClient, DeviceType};
use futures_util::StreamExt;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{debug, info, warn};
use uuid::Uuid;

const MANUFACTURER_ID: u16 = 0xFFFF;
const MIN_COMPATIBLE_VERSION: u8 = 1;
const PAYLOAD_MIN_LEN_V1: usize = 1 + 2 + 16 + 1 + 4;
const PAYLOAD_MIN_LEN_V2: usize = 1 + 2 + 16;
const PAYLOAD_MIN_LEN_V3: usize = 1 + 2 + 16 + 1;

pub struct ProximityHandle {
    shutdown_tx: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

impl ProximityHandle {
    pub fn stop(self) {
        let _ = self.shutdown_tx.send(true);
        self.task.abort();
    }
}

pub fn start(client: Arc<ConnectedClient>) -> Option<ProximityHandle> {
    #[cfg(all(target_os = "linux", feature = "proximity-ble"))]
    {
        return start_linux(client);
    }
    #[cfg(not(all(target_os = "linux", feature = "proximity-ble")))]
    {
        let _ = client;
        None
    }
}

#[cfg(all(target_os = "linux", feature = "proximity-ble"))]
fn start_linux(client: Arc<ConnectedClient>) -> Option<ProximityHandle> {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let task = tokio::spawn(async move {
        run_ble_scan(client, shutdown_rx).await;
    });
    Some(ProximityHandle { shutdown_tx, task })
}

#[cfg(all(target_os = "linux", feature = "proximity-ble"))]
async fn run_ble_scan(client: Arc<ConnectedClient>, mut shutdown_rx: watch::Receiver<bool>) {
    let manager = match Manager::new().await {
        Ok(manager) => manager,
        Err(e) => {
            warn!("BLE manager init failed: {}", e);
            return;
        }
    };

    let adapters = match manager.adapters().await {
        Ok(adapters) => adapters,
        Err(e) => {
            warn!("BLE adapters unavailable: {}", e);
            return;
        }
    };

    let Some(adapter) = adapters.into_iter().next() else {
        warn!("No BLE adapters found");
        return;
    };

    if let Err(e) = adapter.start_scan(ScanFilter::default()).await {
        warn!("BLE scan start failed: {}", e);
        return;
    }

    info!("BLE proximity scan started");

    let mut events = match adapter.events().await {
        Ok(stream) => stream,
        Err(e) => {
            warn!("BLE event stream failed: {}", e);
            let _ = adapter.stop_scan().await;
            return;
        }
    };

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
            event = events.next() => {
                let Some(event) = event else {
                    break;
                };
                handle_event(&client, event);
            }
        }
    }

    let _ = adapter.stop_scan().await;
    info!("BLE proximity scan stopped");
}

#[cfg(all(target_os = "linux", feature = "proximity-ble"))]
fn handle_event(client: &Arc<ConnectedClient>, event: CentralEvent) {
    let CentralEvent::ManufacturerDataAdvertisement {
        manufacturer_data, ..
    } = event
    else {
        return;
    };

    let Some(data) = manufacturer_data.get(&MANUFACTURER_ID) else {
        return;
    };

    let Some(payload) = parse_payload(data) else {
        return;
    };

    if payload.device_id == client.local_device().id {
        return;
    }

    if let Some(ip) = payload.ip {
        if let Err(e) = client.inject_proximity_device(
            payload.device_id,
            payload.name,
            payload.device_type,
            ip,
            payload.port,
        ) {
            debug!("Proximity inject failed: {}", e);
        }
    } else {
        // Protocol >= 2 (Offline/BLE-only mode)
        // Inject with placeholder IP to make it visible in UI
        debug!("Proximity payload missing IP; injecting as offline device");
        if let Err(e) = client.inject_proximity_device(
            payload.device_id,
            payload.name,
            payload.device_type,
            "0.0.0.0".parse().unwrap(),
            payload.port,
        ) {
            debug!("Proximity inject failed: {}", e);
        }
    }
}

#[cfg(all(target_os = "linux", feature = "proximity-ble"))]
struct ProximityPayload {
    device_id: String,
    name: String,
    device_type: DeviceType,
    ip: Option<IpAddr>,
    port: u16,
}

#[cfg(all(target_os = "linux", feature = "proximity-ble"))]
fn parse_payload(data: &[u8]) -> Option<ProximityPayload> {
    if data.len() < PAYLOAD_MIN_LEN_V2 {
        return None;
    }

    let flags = data[0];
    let protocol = (flags >> 4) & 0x0F;
    if protocol < MIN_COMPATIBLE_VERSION {
        return None;
    }

    let device_type_code = flags & 0x0F;
    let port = u16::from_be_bytes([data[1], data[2]]);

    let uuid = Uuid::from_slice(&data[3..19]).ok()?.to_string();

    if protocol >= 2 {
        if protocol >= 3 && data.len() < PAYLOAD_MIN_LEN_V3 {
            return None;
        }
        let device_type = match device_type_code {
            1 => DeviceType::Android,
            2 => DeviceType::Linux,
            3 => DeviceType::Windows,
            4 => DeviceType::MacOS,
            _ => DeviceType::Unknown,
        };

        return Some(ProximityPayload {
            device_id: uuid,
            name: "Unknown".to_string(),
            device_type,
            ip: None,
            port,
        });
    }

    if data.len() < PAYLOAD_MIN_LEN_V1 {
        return None;
    }

    let name_len = data[19] as usize;
    let name_start = 20;
    let name_end = name_start + name_len;
    if data.len() < name_end + 4 {
        return None;
    }

    let name = String::from_utf8_lossy(&data[name_start..name_end]).to_string();
    let ip_bytes = &data[name_end..name_end + 4];
    let ip = IpAddr::V4(Ipv4Addr::new(
        ip_bytes[0],
        ip_bytes[1],
        ip_bytes[2],
        ip_bytes[3],
    ));

    let device_type = match device_type_code {
        1 => DeviceType::Android,
        2 => DeviceType::Linux,
        3 => DeviceType::Windows,
        4 => DeviceType::MacOS,
        _ => DeviceType::Unknown,
    };

    Some(ProximityPayload {
        device_id: uuid,
        name,
        device_type,
        ip: Some(ip),
        port,
    })
}
