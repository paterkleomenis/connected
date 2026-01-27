use crate::device::{Device, DeviceType};
use crate::error::{ConnectedError, Result};
use mdns_sd::{IfKind, ResolvedService, ServiceDaemon, ServiceEvent, ServiceInfo};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, error, info, trace, warn};

// QUIC runs over UDP, so we use UDP service type
const SERVICE_TYPE: &str = "_connected._udp.local.";
const REANNOUNCE_INTERVAL: Duration = Duration::from_secs(5);
const BROWSE_TIMEOUT: Duration = Duration::from_millis(100);
const DEVICE_STALE_TIMEOUT: Duration = Duration::from_secs(15);
const CLEANUP_INTERVAL: Duration = Duration::from_secs(2);
const PROTOCOL_VERSION: u32 = 1;
const MIN_COMPATIBLE_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiscoverySource {
    Connected,
    Discovered,
}

#[derive(Clone)]
struct TrackedEndpoint {
    device: Device,
    last_seen: Instant,
    ip: String, // Cached IP for deduplication
}

#[derive(Clone, Default)]
struct TrackedDevice {
    connected: Option<TrackedEndpoint>,
    discovered: Option<TrackedEndpoint>,
}

impl TrackedDevice {
    fn active_source(&self) -> Option<DiscoverySource> {
        if self.connected.is_some() {
            Some(DiscoverySource::Connected)
        } else if self.discovered.is_some() {
            Some(DiscoverySource::Discovered)
        } else {
            None
        }
    }

    fn active_device(&self) -> Option<Device> {
        self.connected
            .as_ref()
            .or(self.discovered.as_ref())
            .map(|endpoint| endpoint.device.clone())
    }
}

pub struct DiscoveryService {
    daemon: ServiceDaemon,
    local_device: Device,
    discovered_devices: Arc<RwLock<HashMap<String, TrackedDevice>>>,
    running: Arc<AtomicBool>,
    announced: Arc<AtomicBool>,
    service_fullname: String,
    thread_handles: RwLock<Vec<JoinHandle<()>>>,
}

#[derive(Debug, Clone)]
pub enum DiscoveryEvent {
    DeviceFound(Device),
    DeviceLost(String),
    Error(String),
}

impl DiscoveryService {
    pub fn new(local_device: Device) -> Result<Self> {
        let daemon = ServiceDaemon::new()?;

        // Disable virtual network interfaces that can interfere with mDNS multicast
        // This is especially important on Windows where VMware, VirtualBox, Hyper-V,
        // WSL, and Docker create virtual adapters that can capture/misdirect multicast traffic
        Self::disable_virtual_interfaces(&daemon);

        // Create a consistent instance name that we can use for unregistration
        let instance_name = Self::create_instance_name(&local_device);
        let service_fullname = format!("{}.{}", instance_name, SERVICE_TYPE);

        Ok(Self {
            daemon,
            local_device,
            discovered_devices: Arc::new(RwLock::new(HashMap::new())),
            running: Arc::new(AtomicBool::new(false)),
            announced: Arc::new(AtomicBool::new(false)),
            service_fullname,
            thread_handles: RwLock::new(Vec::new()),
        })
    }

    /// Disable virtual network interfaces that commonly interfere with mDNS discovery.
    /// These include VMware, VirtualBox, Hyper-V, WSL, Docker, and other virtualization adapters.
    fn disable_virtual_interfaces(daemon: &ServiceDaemon) {
        // Common virtual interface name patterns to exclude
        let virtual_interface_patterns = [
            // VMware
            "vmnet",
            "vmware",
            // VirtualBox
            "virtualbox",
            "vboxnet",
            // Hyper-V
            "vethernet",
            "hyper-v",
            // WSL
            "wsl",
            // Docker
            "docker",
            "br-",
            "veth",
            // Other common virtual adapters
            "virbr",
            "lxcbr",
            "lxdbr",
            "podman",
            "cni",
            "flannel",
            "calico",
            "weave",
            // Loopback (already excluded by mdns-sd, but be explicit)
            "loopback",
            // Bluetooth PAN
            "bluetooth",
            // VPN adapters
            "tap-",
            "tun",
            "utun",
            "pptp",
            "ipsec",
            "wireguard",
            "wg",
            "nordlynx",
            "proton",
            "mullvad",
        ];

        for pattern in virtual_interface_patterns {
            if let Err(e) = daemon.disable_interface(IfKind::Name(pattern.to_string())) {
                // Log at trace level since many of these interfaces won't exist on most systems
                trace!("Could not disable interface pattern '{}': {}", pattern, e);
            }
        }

        debug!("Disabled virtual network interfaces for mDNS daemon");
    }

    fn create_instance_name(device: &Device) -> String {
        // Use a format that's easy to parse: name--uuid (double dash as separator)
        // This avoids issues with single underscores in device names
        format!("{}--{}", device.name.replace("--", "-"), device.id)
    }

    fn parse_instance_name(instance: &str) -> Option<(String, String)> {
        // Parse "name--uuid" format
        if let Some(pos) = instance.rfind("--") {
            let name = instance[..pos].to_string();
            let id = instance[pos + 2..].to_string();
            if !id.is_empty() && !name.is_empty() {
                return Some((name, id));
            }
        }
        None
    }

    pub fn announce(&self) -> Result<()> {
        let instance_name = Self::create_instance_name(&self.local_device);

        // Properties for service discovery
        let mut properties = HashMap::new();
        properties.insert("id".to_string(), self.local_device.id.clone());
        properties.insert("name".to_string(), self.local_device.name.clone());
        properties.insert(
            "type".to_string(),
            self.local_device.device_type.as_str().to_string(),
        );
        properties.insert("version".to_string(), PROTOCOL_VERSION.to_string());

        let ip: IpAddr = self
            .local_device
            .ip
            .parse()
            .map_err(|_| ConnectedError::InvalidAddress(self.local_device.ip.clone()))?;

        if ip.is_unspecified() {
            warn!("Local IP unspecified; skipping mDNS announce");
            return Ok(());
        }

        // Create the hostname - use a unique name for this device
        let hostname = format!("{}.local.", self.local_device.id);

        debug!(
            "Creating mDNS service: type={}, instance={}, hostname={}, ip={}, port={}",
            SERVICE_TYPE, instance_name, hostname, ip, self.local_device.port
        );

        let service_info = ServiceInfo::new(
            SERVICE_TYPE,
            &instance_name,
            &hostname,
            ip,
            self.local_device.port,
            properties.clone(),
        )?
        .enable_addr_auto();

        debug!("Service properties: {:?}", properties);

        // Register (will update if already registered)
        self.daemon.register(service_info)?;
        self.announced.store(true, Ordering::SeqCst);

        info!(
            "Announced device '{}' (id={}) on mDNS at {}:{} [service_type={}]",
            self.local_device.name,
            self.local_device.id,
            self.local_device.ip,
            self.local_device.port,
            SERVICE_TYPE
        );

        Ok(())
    }

    pub fn start_listening(&self, event_tx: mpsc::UnboundedSender<DiscoveryEvent>) -> Result<()> {
        // If already running, just re-announce
        if self.running.swap(true, Ordering::SeqCst) {
            debug!("Discovery already running, re-announcing");
            let _ = self.announce();
            return Ok(());
        }

        // Clear any stale devices from previous sessions
        self.clear_discovered_devices();

        // Start browsing for services
        let receiver = self.daemon.browse(SERVICE_TYPE)?;
        let local_id = self.local_device.id.clone();
        let discovered = self.discovered_devices.clone();
        let running = self.running.clone();
        let event_tx_discovery = event_tx.clone();

        info!("Started mDNS discovery for {}", SERVICE_TYPE);

        // Main discovery thread
        let discovery_handle = std::thread::Builder::new()
            .name("mdns-discovery".to_string())
            .spawn(move || {
                while running.load(Ordering::SeqCst) {
                    match receiver.recv_timeout(BROWSE_TIMEOUT) {
                        Ok(event) => {
                            Self::handle_event(event, &local_id, &discovered, &event_tx_discovery);
                        }
                        Err(e) => {
                            // Check if it's a timeout (normal) or disconnected (error)
                            let err_msg = e.to_string().to_lowercase();
                            if err_msg.contains("timeout") || err_msg.contains("timed out") {
                                // Normal timeout, continue polling
                                continue;
                            }
                            // Channel disconnected or other error
                            warn!("mDNS receiver error: {}", e);
                            break;
                        }
                    }
                }
                info!("Discovery listener stopped");
            })?;

        // Periodic re-announcement thread for visibility
        let daemon = self.daemon.clone();
        let local_device = self.local_device.clone();
        let running_announce = self.running.clone();
        let announced = self.announced.clone();

        let announce_handle = std::thread::Builder::new()
            .name("mdns-announce".to_string())
            .spawn(move || {
                // Initial delay before first re-announcement
                std::thread::sleep(REANNOUNCE_INTERVAL / 2);

                while running_announce.load(Ordering::SeqCst) {
                    if announced.load(Ordering::SeqCst) {
                        match Self::do_announce(&daemon, &local_device) {
                            Err(e) => {
                                debug!("Re-announcement failed: {}", e);
                            }
                            _ => {
                                debug!("Re-announced device on mDNS");
                            }
                        }
                    }
                    std::thread::sleep(REANNOUNCE_INTERVAL);
                }
                debug!("Announce thread stopped");
            })?;

        // Periodic cleanup thread to remove stale devices
        let discovered_cleanup = self.discovered_devices.clone();
        let running_cleanup = self.running.clone();
        let event_tx_cleanup = event_tx.clone();

        let cleanup_handle = std::thread::Builder::new()
            .name("mdns-cleanup".to_string())
            .spawn(move || {
                while running_cleanup.load(Ordering::SeqCst) {
                    std::thread::sleep(CLEANUP_INTERVAL);

                    if !running_cleanup.load(Ordering::SeqCst) {
                        break;
                    }

                    let now = Instant::now();
                    let events = {
                        let mut devices = discovered_cleanup.write();
                        Self::cleanup_stale_endpoints(&mut devices, now)
                    };

                    for event in events {
                        let _ = event_tx_cleanup.send(event);
                    }
                }
                debug!("Device cleanup thread stopped");
            })?;

        // Store thread handles for proper cleanup
        {
            let mut handles = self.thread_handles.write();
            handles.push(discovery_handle);
            handles.push(announce_handle);
            handles.push(cleanup_handle);
        }

        Ok(())
    }

    fn do_announce(daemon: &ServiceDaemon, device: &Device) -> Result<()> {
        let instance_name = Self::create_instance_name(device);
        let hostname = format!("{}.local.", device.id);

        let mut properties = HashMap::new();
        properties.insert("id".to_string(), device.id.clone());
        properties.insert("name".to_string(), device.name.clone());
        properties.insert("type".to_string(), device.device_type.as_str().to_string());
        properties.insert("version".to_string(), PROTOCOL_VERSION.to_string());

        let ip: IpAddr = device
            .ip
            .parse()
            .map_err(|_| ConnectedError::InvalidAddress(device.ip.clone()))?;

        if ip.is_unspecified() {
            debug!("Local IP unspecified; skipping mDNS re-announce");
            return Ok(());
        }

        let service_info = ServiceInfo::new(
            SERVICE_TYPE,
            &instance_name,
            &hostname,
            ip,
            device.port,
            properties,
        )?
        .enable_addr_auto();

        daemon.register(service_info)?;
        Ok(())
    }

    pub(crate) fn upsert_device_endpoint(
        &self,
        device: Device,
        source: DiscoverySource,
    ) -> Option<DiscoveryEvent> {
        let mut devices = self.discovered_devices.write();
        Self::upsert_endpoint_locked(&mut devices, device, source)
    }

    fn transition_event(
        device_id: &str,
        prev_source: Option<DiscoverySource>,
        prev_device: Option<Device>,
        new_source: Option<DiscoverySource>,
        new_device: Option<Device>,
    ) -> Option<DiscoveryEvent> {
        match (prev_device, new_device) {
            (None, Some(device)) => Some(DiscoveryEvent::DeviceFound(device)),
            (Some(_), None) => Some(DiscoveryEvent::DeviceLost(device_id.to_string())),
            (Some(prev), Some(curr)) => {
                let source_changed = prev_source != new_source;
                let discovered_changed =
                    new_source == Some(DiscoverySource::Discovered) && prev != curr;

                if source_changed || discovered_changed {
                    Some(DiscoveryEvent::DeviceFound(curr))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn upsert_endpoint_locked(
        devices: &mut HashMap<String, TrackedDevice>,
        device: Device,
        source: DiscoverySource,
    ) -> Option<DiscoveryEvent> {
        let device_id = device.id.clone();
        let tracked = devices.entry(device_id.clone()).or_default();
        let prev_source = tracked.active_source();
        let prev_device = tracked.active_device();

        let endpoint = TrackedEndpoint {
            device: device.clone(),
            last_seen: Instant::now(),
            ip: device.ip.clone(),
        };

        match source {
            DiscoverySource::Connected => tracked.connected = Some(endpoint),
            DiscoverySource::Discovered => {
                // If we already have a discovered endpoint with a valid IP,
                // and the new one has an unspecified IP (0.0.0.0),
                // we should NOT overwrite the valid IP.
                // This happens when Proximity (BLE) detects a device but hasn't resolved IP yet,
                // while mDNS has already found the valid IP.
                let should_update = if let Some(existing) = &tracked.discovered {
                    let new_is_unspecified = device.ip == "0.0.0.0" || device.ip == "::";
                    let existing_is_valid =
                        existing.device.ip != "0.0.0.0" && existing.device.ip != "::";

                    if new_is_unspecified && existing_is_valid {
                        // Just update last_seen to keep it alive, but don't overwrite device info
                        tracked.discovered.as_mut().unwrap().last_seen = Instant::now();
                        false
                    } else {
                        true
                    }
                } else {
                    true
                };

                if should_update {
                    tracked.discovered = Some(endpoint);
                }
            }
        }

        let new_source = tracked.active_source();
        let new_device = tracked.active_device();

        Self::transition_event(&device_id, prev_source, prev_device, new_source, new_device)
    }

    fn remove_endpoint_locked(
        devices: &mut HashMap<String, TrackedDevice>,
        device_id: &str,
        source: DiscoverySource,
    ) -> Option<DiscoveryEvent> {
        let tracked = devices.get_mut(device_id)?;

        let prev_source = tracked.active_source();
        let prev_device = tracked.active_device();

        match source {
            DiscoverySource::Connected => tracked.connected = None,
            DiscoverySource::Discovered => tracked.discovered = None,
        }

        let new_source = tracked.active_source();
        let new_device = tracked.active_device();

        if tracked.connected.is_none() && tracked.discovered.is_none() {
            devices.remove(device_id);
        }

        Self::transition_event(device_id, prev_source, prev_device, new_source, new_device)
    }

    fn cleanup_stale_endpoints(
        devices: &mut HashMap<String, TrackedDevice>,
        now: Instant,
    ) -> Vec<DiscoveryEvent> {
        let mut events = Vec::new();
        let ids: Vec<String> = devices.keys().cloned().collect();

        for device_id in ids {
            let Some(tracked) = devices.get_mut(&device_id) else {
                continue;
            };

            let prev_source = tracked.active_source();
            let prev_device = tracked.active_device();

            if tracked.connected.as_ref().is_some_and(|endpoint| {
                now.duration_since(endpoint.last_seen) > DEVICE_STALE_TIMEOUT
            }) {
                tracked.connected = None;
            }

            if tracked.discovered.as_ref().is_some_and(|endpoint| {
                now.duration_since(endpoint.last_seen) > DEVICE_STALE_TIMEOUT
            }) {
                tracked.discovered = None;
            }

            let new_source = tracked.active_source();

            let new_device = tracked.active_device();

            if tracked.connected.is_none() && tracked.discovered.is_none() {
                devices.remove(&device_id);
            }

            if let Some(event) =
                Self::transition_event(&device_id, prev_source, prev_device, new_source, new_device)
            {
                events.push(event);
            }
        }

        events
    }

    fn handle_event(
        event: ServiceEvent,
        local_id: &str,
        discovered: &Arc<RwLock<HashMap<String, TrackedDevice>>>,
        event_tx: &mpsc::UnboundedSender<DiscoveryEvent>,
    ) {
        match event {
            ServiceEvent::ServiceResolved(info) => {
                trace!("ServiceResolved event received: {}", info.fullname);
                Self::handle_service_resolved(&info, local_id, discovered, event_tx);
            }
            ServiceEvent::ServiceRemoved(_service_type, fullname) => {
                debug!("ServiceRemoved event: {}", fullname);
                Self::handle_service_removed(&fullname, discovered, event_tx);
            }
            ServiceEvent::SearchStarted(stype) => {
                debug!("mDNS search started for {}", stype);
            }
            ServiceEvent::SearchStopped(stype) => {
                debug!("mDNS search stopped for {}", stype);
            }
            ServiceEvent::ServiceFound(stype, fullname) => {
                info!(
                    "mDNS ServiceFound: {} - {} (will resolve...)",
                    stype, fullname
                );
            }
            _ => {
                trace!("Unhandled mDNS event");
            }
        }
    }

    fn handle_service_resolved(
        info: &ResolvedService,
        local_id: &str,
        discovered: &Arc<RwLock<HashMap<String, TrackedDevice>>>,
        event_tx: &mpsc::UnboundedSender<DiscoveryEvent>,
    ) {
        info!(
            "ServiceResolved: fullname={}, addresses={:?}, port={}",
            info.fullname, info.addresses, info.port
        );

        // Log all TXT properties for debugging
        for prop in info.txt_properties.iter() {
            debug!("  TXT property: {}={}", prop.key(), prop.val_str());
        }

        // Check protocol version compatibility
        let version = info
            .txt_properties
            .get("version")
            .and_then(|v| v.val_str().parse::<u32>().ok())
            .unwrap_or(0);

        if version < MIN_COMPATIBLE_VERSION {
            warn!(
                "Ignoring device with incompatible protocol version {} (min: {}): {}",
                version, MIN_COMPATIBLE_VERSION, info.fullname
            );
            return;
        }

        if version > PROTOCOL_VERSION {
            info!(
                "Device {} has newer protocol version {} (ours: {}), may have reduced functionality",
                info.fullname, version, PROTOCOL_VERSION
            );
        }

        // Try to get device ID from TXT records first (most reliable)
        let device_id = info
            .txt_properties
            .get("id")
            .map(|v| v.val_str().to_string())
            .unwrap_or_default();

        // If not in TXT records, parse from instance name
        let device_id = if device_id.is_empty() {
            debug!(
                "No 'id' in TXT records, parsing from fullname: {}",
                info.fullname
            );
            // fullname format: "name--uuid._connected._udp.local."
            if let Some(instance) = info.fullname.strip_suffix(&format!(".{}", SERVICE_TYPE)) {
                debug!("Stripped suffix, instance: {}", instance);
                Self::parse_instance_name(instance)
                    .map(|(_, id)| id)
                    .unwrap_or_default()
            } else if let Some(instance) = info.fullname.split('.').next() {
                debug!("Split by dot, instance: {}", instance);
                Self::parse_instance_name(instance)
                    .map(|(_, id)| id)
                    .unwrap_or_default()
            } else {
                String::new()
            }
        } else {
            debug!("Got device_id from TXT records: {}", device_id);
            device_id
        };

        // Skip if this is our own device or invalid
        if device_id.is_empty() {
            warn!(
                "Could not extract device ID from service info: {}",
                info.fullname
            );
            return;
        }
        if device_id == local_id {
            trace!("Ignoring self (local device id={})", local_id);
            return;
        }

        debug!("Processing remote device: id={}", device_id);

        // Get device name from TXT or parse from instance
        let device_name = info
            .txt_properties
            .get("name")
            .map(|v| v.val_str().to_string())
            .unwrap_or_else(|| {
                if let Some(instance) = info.fullname.strip_suffix(&format!(".{}", SERVICE_TYPE)) {
                    Self::parse_instance_name(instance)
                        .map(|(name, _)| name)
                        .unwrap_or_else(|| "Unknown".to_string())
                } else {
                    "Unknown".to_string()
                }
            });

        let device_type = info
            .txt_properties
            .get("type")
            .map(|v| v.val_str().parse().unwrap_or(DeviceType::Unknown))
            .unwrap_or(DeviceType::Unknown);

        // Get best IP address (prefer IPv4 for compatibility)
        // In mdns-sd 0.17, addresses are ScopedIp which contains the IpAddr
        let ip = info
            .addresses
            .iter()
            .find(|scoped_ip| scoped_ip.is_ipv4())
            .or_else(|| info.addresses.iter().next())
            .map(|scoped_ip| scoped_ip.to_ip_addr());

        let Some(ip) = ip else {
            warn!("No IP address found for device: {}", device_name);
            return;
        };

        let device = Device::new(
            device_id.clone(),
            device_name.clone(),
            ip,
            info.port,
            device_type,
        );

        let ip_str = ip.to_string();

        info!(
            "Discovered device: {} ({}) at {}:{}",
            device_name, device_id, ip, info.port
        );

        // Check if this is a new device or an update
        // Also check for IP conflicts (same IP, different ID = device restarted)
        let (event, old_device_to_remove) = {
            let mut devices = discovered.write();

            // Check if there's an existing device with same IP but different ID
            // This happens when a device restarts and gets a new UUID
            let old_id_with_same_ip: Option<String> = devices
                .iter()
                .find(|(id, tracked)| {
                    tracked
                        .discovered
                        .as_ref()
                        .is_some_and(|endpoint| endpoint.ip == ip_str)
                        && *id != &device_id
                })
                .map(|(id, _)| id.clone());

            // Remove old entry with same IP if exists
            if let Some(ref old_id) = old_id_with_same_ip {
                info!(
                    "Removing stale device entry {} (same IP {} as new device {})",
                    old_id, ip_str, device_id
                );
                devices.remove(old_id);
            }

            (
                Self::upsert_endpoint_locked(
                    &mut devices,
                    device.clone(),
                    DiscoverySource::Discovered,
                ),
                old_id_with_same_ip,
            )
        };

        // Notify about removed device if we replaced one
        if let Some(old_id) = old_device_to_remove {
            let _ = event_tx.send(DiscoveryEvent::DeviceLost(old_id));
        }

        if let Some(event) = event {
            if let DiscoveryEvent::DeviceFound(_) = &event {
                info!(
                    "Device active endpoint updated: {} ({}) at {}:{}",
                    device_name, device_id, ip, info.port
                );
            }
            if event_tx.send(event).is_err() {
                error!("Event channel closed - cannot notify about device update!");
            }
        } else {
            trace!("Updated existing device: {} ({})", device_name, device_id);
        }
    }

    fn handle_service_removed(
        fullname: &str,
        discovered: &Arc<RwLock<HashMap<String, TrackedDevice>>>,
        event_tx: &mpsc::UnboundedSender<DiscoveryEvent>,
    ) {
        debug!("ServiceRemoved: {}", fullname);

        // Parse device ID from fullname: "name--uuid._connected._udp.local."
        let device_id = if let Some(instance) = fullname.strip_suffix(&format!(".{}", SERVICE_TYPE))
        {
            Self::parse_instance_name(instance).map(|(_, id)| id)
        } else if let Some(instance) = fullname.split('.').next() {
            Self::parse_instance_name(instance).map(|(_, id)| id)
        } else {
            None
        };

        let Some(device_id) = device_id else {
            debug!(
                "Could not parse device ID from removed service: {}",
                fullname
            );
            return;
        };

        let event = {
            let mut devices = discovered.write();
            Self::remove_endpoint_locked(&mut devices, &device_id, DiscoverySource::Discovered)
        };

        if let Some(event) = event {
            if let DiscoveryEvent::DeviceLost(_) = &event {
                info!("Device left: ({})", device_id);
            }
            let _ = event_tx.send(event);
        }
    }

    pub fn shutdown(&self) {
        // Stop running flag first to signal threads to exit
        self.running.store(false, Ordering::SeqCst);
        self.announced.store(false, Ordering::SeqCst);

        // Stop browsing first (this closes the browse receiver)
        if let Err(e) = self.daemon.stop_browse(SERVICE_TYPE) {
            debug!("Failed to stop mDNS browse: {}", e);
        }

        // Unregister our service
        if let Err(e) = self.daemon.unregister(&self.service_fullname) {
            debug!("Failed to unregister mDNS service: {}", e);
        }

        // Wait for threads to finish with timeout
        let handles: Vec<JoinHandle<()>> = {
            let mut guard = self.thread_handles.write();
            std::mem::take(&mut *guard)
        };

        for handle in handles {
            let thread_name = handle.thread().name().unwrap_or("unnamed").to_string();
            match handle.join() {
                Ok(()) => debug!("Thread '{}' joined successfully", thread_name),
                Err(_) => warn!("Thread '{}' panicked during shutdown", thread_name),
            }
        }

        // Give time for goodbye packets
        std::thread::sleep(Duration::from_millis(50));

        // Shutdown daemon
        if let Err(e) = self.daemon.shutdown() {
            debug!("Failed to shutdown mDNS daemon: {}", e);
        }

        info!("Discovery service shut down");
    }

    pub fn get_discovered_devices(&self) -> Vec<Device> {
        self.discovered_devices
            .read()
            .values()
            .filter_map(|tracked| tracked.active_device())
            .collect()
    }

    pub fn get_device_by_id(&self, id: &str) -> Option<Device> {
        self.discovered_devices
            .read()
            .get(id)
            .and_then(|tracked| tracked.active_device())
    }

    pub fn clear_discovered_devices(&self) {
        let mut devices = self.discovered_devices.write();
        let count = devices.len();
        devices.clear();
        if count > 0 {
            info!("Cleared {} discovered devices", count);
        }
    }
}

impl Drop for DiscoveryService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instance_name_parsing() {
        // Standard case
        let (name, id) = DiscoveryService::parse_instance_name("MyDevice--abc123").unwrap();
        assert_eq!(name, "MyDevice");
        assert_eq!(id, "abc123");

        // Name with single dash
        let (name, id) = DiscoveryService::parse_instance_name("My-Device--abc123").unwrap();
        assert_eq!(name, "My-Device");
        assert_eq!(id, "abc123");

        // Name with underscore
        let (name, id) = DiscoveryService::parse_instance_name("My_Device--abc123").unwrap();
        assert_eq!(name, "My_Device");
        assert_eq!(id, "abc123");

        // UUID-like ID
        let (name, id) =
            DiscoveryService::parse_instance_name("Desktop--550e8400-e29b-41d4-a716-446655440000")
                .unwrap();
        assert_eq!(name, "Desktop");
        assert_eq!(id, "550e8400-e29b-41d4-a716-446655440000");

        // Invalid cases
        assert!(DiscoveryService::parse_instance_name("NoSeparator").is_none());
        assert!(DiscoveryService::parse_instance_name("--OnlyId").is_none());
        assert!(DiscoveryService::parse_instance_name("OnlyName--").is_none());
    }

    #[test]
    fn test_create_instance_name() {
        use std::net::Ipv4Addr;

        let device = Device::new(
            "test-id-123".to_string(),
            "TestDevice".to_string(),
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)),
            44444,
            DeviceType::Linux,
        );

        let instance_name = DiscoveryService::create_instance_name(&device);
        assert_eq!(instance_name, "TestDevice--test-id-123");

        // Device with double dash in name gets it replaced
        let device2 = Device::new(
            "test-id-456".to_string(),
            "Test--Device".to_string(),
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 2)),
            44444,
            DeviceType::Linux,
        );

        let instance_name2 = DiscoveryService::create_instance_name(&device2);
        assert_eq!(instance_name2, "Test-Device--test-id-456");
    }
}
