use crate::device::{Device, DeviceType};
use crate::error::{ConnectedError, Result};
use mdns_sd::{IfKind, ResolvedService, ScopedIp, ServiceDaemon, ServiceEvent, ServiceInfo};
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
const BROWSE_TIMEOUT: Duration = Duration::from_millis(100);
const DEVICE_STALE_TIMEOUT: Duration = Duration::from_secs(10);
const CLEANUP_INTERVAL: Duration = Duration::from_secs(5);
const REANNOUNCE_INTERVAL: Duration = Duration::from_secs(5);
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
    local_name: Arc<RwLock<String>>,
    discovered_devices: Arc<RwLock<HashMap<String, TrackedDevice>>>,
    running: Arc<AtomicBool>,
    announced: Arc<AtomicBool>,
    service_fullname: Arc<RwLock<String>>,
    browse_event_tx: RwLock<Option<mpsc::UnboundedSender<DiscoveryEvent>>>,
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
        let daemon = match ServiceDaemon::new() {
            Ok(d) => d,
            Err(e) => {
                #[cfg(target_os = "windows")]
                error!(
                    "mDNS ServiceDaemon failed to start. \
                     This usually means Windows Firewall or another security \
                     product is blocking UDP port 5353. \
                     Run the following in an admin PowerShell to add the required rules:\n\
                     netsh advfirewall firewall add rule name=\"Connected (mDNS-In)\" \
                       dir=in protocol=udp localport=5353 action=allow\n\
                     netsh advfirewall firewall add rule name=\"Connected (mDNS-Out)\" \
                       dir=out protocol=udp remoteport=5353 action=allow\n\
                     Error: {}",
                    e
                );
                return Err(ConnectedError::Discovery(format!(
                    "mDNS daemon failed: {}. \
                     On Windows, ensure firewall allows UDP port 5353 \
                     (run Connected Desktop once as admin to auto-configure).",
                    e
                )));
            }
        };

        // Disable virtual network interfaces that can interfere with mDNS multicast
        // This is especially important on Windows where VMware, VirtualBox, Hyper-V,
        // WSL, and Docker create virtual adapters that can capture/misdirect multicast traffic
        Self::disable_virtual_interfaces(&daemon);

        // Create a consistent instance name that we can use for unregistration
        let instance_name = Self::create_instance_name(&local_device);
        let service_fullname = format!("{}.{}", instance_name, SERVICE_TYPE);

        Ok(Self {
            daemon,
            local_name: Arc::new(RwLock::new(local_device.name.clone())),
            local_device,
            discovered_devices: Arc::new(RwLock::new(HashMap::new())),
            running: Arc::new(AtomicBool::new(false)),
            announced: Arc::new(AtomicBool::new(false)),
            service_fullname: Arc::new(RwLock::new(service_fullname)),
            browse_event_tx: RwLock::new(None),
            thread_handles: RwLock::new(Vec::new()),
        })
    }

    /// Disable virtual network interfaces that commonly interfere with mDNS discovery.
    /// These include VMware, VirtualBox, Hyper-V, WSL, Docker, and other virtualization adapters.
    fn disable_virtual_interfaces(daemon: &ServiceDaemon) {
        fn disable_interface_name(daemon: &ServiceDaemon, name: &str) {
            if let Err(e) = daemon.disable_interface(IfKind::Name(name.to_string())) {
                trace!("Could not disable interface '{}': {}", name, e);
            }
        }

        // Common virtual interface name patterns to exclude
        let virtual_interface_patterns: &[&str] = &[
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
            // Android cellular / test / peer-to-peer interfaces. These are not
            // useful for LAN mDNS and can produce unroutable peer endpoints.
            "rmnet",
            "r_rmnet",
            "ccmni",
            "rev_rmnet",
            "dummy",
            "p2p",
            "clat",
        ];

        // Register Android interface names up front. Some of these interfaces can
        // appear after the daemon starts, so relying only on the current if_addrs
        // snapshot leaves mDNS on unreachable cellular or Wi-Fi Direct adapters.
        for name in ["dummy0", "p2p0", "p2p-p2p0", "clat4", "v4-rmnet_data0"] {
            disable_interface_name(daemon, name);
        }
        for idx in 0..16 {
            for prefix in [
                "rmnet_data",
                "r_rmnet_data",
                "rev_rmnet_data",
                "ccmni",
                "ccmni_data",
                "v4-rmnet_data",
            ] {
                disable_interface_name(daemon, &format!("{}{}", prefix, idx));
            }
        }

        // Enumerate real OS interfaces and disable any whose name matches a
        // virtual-interface pattern (case-insensitive substring).
        // This is needed because IfKind::Name is case-sensitive.
        match if_addrs::get_if_addrs() {
            Ok(interfaces) => {
                // Deduplicate: an interface can appear multiple times (once per
                // address family) but we only need to disable it once.
                let mut seen = std::collections::HashSet::new();

                for iface in &interfaces {
                    if !seen.insert(iface.name.clone()) {
                        continue;
                    }

                    let name_lower = iface.name.to_lowercase();
                    let dominated = virtual_interface_patterns
                        .iter()
                        .any(|pat| name_lower.contains(&pat.to_lowercase()));

                    if dominated {
                        info!(
                            "Disabling virtual interface '{}' (addr {})",
                            iface.name,
                            iface.ip()
                        );
                        if let Err(e) = daemon.disable_interface(IfKind::Name(iface.name.clone())) {
                            warn!("Failed to disable interface '{}': {}", iface.name, e);
                        }
                    }
                }
            }
            Err(e) => {
                warn!(
                    "Could not enumerate network interfaces for virtual-adapter \
                     filtering: {}. mDNS may use unintended adapters.",
                    e
                );
            }
        }

        // Also disable loopback explicitly (by kind, not name).
        if let Err(e) = daemon.disable_interface(IfKind::LoopbackV4) {
            trace!("Could not disable LoopbackV4: {}", e);
        }
        if let Err(e) = daemon.disable_interface(IfKind::LoopbackV6) {
            trace!("Could not disable LoopbackV6: {}", e);
        }

        debug!("Disabled virtual network interfaces for mDNS daemon");
    }

    fn create_instance_name(device: &Device) -> String {
        Self::create_instance_name_with_name(device, &device.name)
    }

    fn create_instance_name_with_name(device: &Device, name: &str) -> String {
        // Use a format that's easy to parse: name--uuid (double dash as separator)
        // This avoids issues with single underscores in device names
        format!("{}--{}", name.replace("--", "-"), device.id)
    }

    fn device_snapshot(local_device: &Device, local_name: &Arc<RwLock<String>>) -> Device {
        let mut device = local_device.clone();
        device.name = local_name.read().clone();
        device
    }

    fn local_device_snapshot(&self) -> Device {
        Self::device_snapshot(&self.local_device, &self.local_name)
    }

    fn mark_announced(&self, instance_name: &str) {
        *self.service_fullname.write() = format!("{}.{}", instance_name, SERVICE_TYPE);
        self.announced.store(true, Ordering::SeqCst);
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
        let device = self.local_device_snapshot();

        Self::do_announce(&self.daemon, &device)?;

        let instance_name = Self::create_instance_name(&device);
        self.mark_announced(&instance_name);

        info!(
            "Announced device '{}' (id={}) on mDNS at {}:{} [service_type={}]",
            device.name, device.id, device.ip, device.port, SERVICE_TYPE
        );

        Ok(())
    }

    fn do_announce(daemon: &ServiceDaemon, device: &Device) -> Result<()> {
        let instance_name = Self::create_instance_name(device);
        let hostname = format!("{}.local.", device.id);

        // Properties for service discovery
        let mut properties = HashMap::new();
        properties.insert("id".to_string(), device.id.clone());
        properties.insert("name".to_string(), device.name.clone());
        properties.insert("type".to_string(), device.device_type.as_str().to_string());
        properties.insert("version".to_string(), PROTOCOL_VERSION.to_string());

        let mut ip: IpAddr = device
            .ip
            .parse()
            .map_err(|_| ConnectedError::InvalidAddress(device.ip.clone()))?;

        if ip.is_unspecified() {
            ip = crate::client::get_local_ip()
                .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
            if ip.is_unspecified() {
                debug!("Local IP unspecified; skipping mDNS re-announce");
                return Ok(());
            }
            debug!("Resolved local IP for mDNS announce: {}", ip);
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

    pub fn start_listening(&self, event_tx: mpsc::UnboundedSender<DiscoveryEvent>) -> Result<()> {
        *self.browse_event_tx.write() = Some(event_tx.clone());

        // If already running, restart browse to force a fresh mDNS query.
        if self.running.swap(true, Ordering::SeqCst) {
            debug!("Discovery already running, restarting browse and re-announcing");
            self.restart_browse()?;
            let _ = self.announce();
            return Ok(());
        }

        // Clear any devices from previous sessions
        self.clear_discovered_devices();

        self.spawn_browse_loop(event_tx)
    }

    fn spawn_browse_loop(&self, event_tx: mpsc::UnboundedSender<DiscoveryEvent>) -> Result<()> {
        // Start browsing for services. The daemon's command queue may be temporarily
        // full during startup while it processes interface setup.
        let receiver = loop {
            match self.daemon.browse(SERVICE_TYPE) {
                Ok(r) => break r,
                Err(mdns_sd::Error::Again) => {
                    debug!("mDNS browse queue full, retrying in 100ms");
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => return Err(e.into()),
            }
        };
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

        self.thread_handles.write().push(discovery_handle);

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

        self.thread_handles.write().push(cleanup_handle);

        // Periodic re-announcement thread to keep our mDNS service alive.
        // mdns-sd does not re-announce automatically; after the TTL expires (~120s),
        // peers forget us. We re-register every REANNOUNCE_INTERVAL.
        let daemon_clone = self.daemon.clone();
        let running_reannounce = self.running.clone();
        let announced_reannounce = self.announced.clone();
        let local_device_reannounce = self.local_device.clone();
        let local_name_reannounce = self.local_name.clone();

        let reannounce_handle = std::thread::Builder::new()
            .name("mdns-reannounce".to_string())
            .spawn(move || {
                // Initial delay to avoid redundant re-announce right after announce().
                std::thread::sleep(REANNOUNCE_INTERVAL / 2);

                while running_reannounce.load(Ordering::SeqCst) {
                    if announced_reannounce.load(Ordering::SeqCst) {
                        let device =
                            Self::device_snapshot(&local_device_reannounce, &local_name_reannounce);
                        match Self::do_announce(&daemon_clone, &device) {
                            Err(e) => debug!("mDNS re-announce failed: {}", e),
                            _ => debug!("Re-announced device on mDNS"),
                        }
                    }
                    std::thread::sleep(REANNOUNCE_INTERVAL);
                }
                debug!("mDNS re-announce thread stopped");
            })?;

        self.thread_handles.write().push(reannounce_handle);

        Ok(())
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

    pub fn restart_browse(&self) -> Result<()> {
        if !self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        let event_tx =
            self.browse_event_tx.read().clone().ok_or_else(|| {
                ConnectedError::Discovery("mDNS browse event channel missing".into())
            })?;

        self.running.store(false, Ordering::SeqCst);

        if let Err(e) = self.daemon.stop_browse(SERVICE_TYPE) {
            debug!("Failed to stop mDNS browse during refresh: {}", e);
        }

        self.join_thread_handles("restarting discovery browse");

        self.running.store(true, Ordering::SeqCst);
        if let Err(e) = self.spawn_browse_loop(event_tx) {
            self.running.store(false, Ordering::SeqCst);
            return Err(e);
        }

        Ok(())
    }

    fn join_thread_handles(&self, context: &str) {
        let handles: Vec<JoinHandle<()>> = {
            let mut guard = self.thread_handles.write();
            std::mem::take(&mut *guard)
        };

        for handle in handles {
            let thread_name = handle.thread().name().unwrap_or("unnamed").to_string();
            match handle.join() {
                Ok(()) => debug!("Thread '{}' joined while {}", thread_name, context),
                Err(_) => warn!("Thread '{}' panicked while {}", thread_name, context),
            }
        }
    }

    pub fn update_local_name(&self, new_name: String) -> Result<()> {
        let current_name = self.local_name.read().clone();
        if current_name == new_name {
            return Ok(());
        }

        let old_fullname = self.service_fullname.read().clone();
        *self.local_name.write() = new_name;

        if self.announced.load(Ordering::SeqCst) {
            if let Err(e) = self.daemon.unregister(&old_fullname) {
                debug!(
                    "Failed to unregister previous mDNS service '{}': {}",
                    old_fullname, e
                );
            }
            self.announce()?;
        }

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
                let device_changed = prev != curr;

                if source_changed || device_changed {
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

        // Preserve known device type across endpoint updates. Connection events often
        // don't include platform info, so avoid downgrading Android/iOS/etc to Unknown.
        let mut device = device;
        if device.device_type == DeviceType::Unknown
            && let Some(existing_type) = tracked
                .connected
                .as_ref()
                .map(|endpoint| endpoint.device.device_type)
                .filter(|dtype| *dtype != DeviceType::Unknown)
                .or_else(|| {
                    tracked
                        .discovered
                        .as_ref()
                        .map(|endpoint| endpoint.device.device_type)
                        .filter(|dtype| *dtype != DeviceType::Unknown)
                })
        {
            device.device_type = existing_type;
        }

        let endpoint = TrackedEndpoint {
            device: device.clone(),
            last_seen: Instant::now(),
            ip: device.ip.clone(),
        };

        match source {
            DiscoverySource::Connected => {
                tracked.connected = Some(endpoint);

                // Keep discovered metadata in sync so source failover does not
                // regress visible device info.
                if let Some(discovered) = tracked.discovered.as_mut() {
                    if discovered.device.name != device.name {
                        discovered.device.name = device.name.clone();
                    }

                    if device.device_type != DeviceType::Unknown
                        && discovered.device.device_type == DeviceType::Unknown
                    {
                        discovered.device.device_type = device.device_type;
                    }
                }
            }
            DiscoverySource::Discovered => {
                // If we already have a discovered endpoint with a valid IP,
                // and the new one has an unspecified IP (0.0.0.0),
                // we should NOT overwrite the valid IP.
                // This can happen when a secondary discovery source reports a
                // placeholder endpoint while mDNS has already found the valid IP.
                if let Some(existing) = tracked.discovered.as_mut() {
                    let new_is_unspecified = device.ip == "0.0.0.0" || device.ip == "::";
                    let existing_is_valid =
                        existing.device.ip != "0.0.0.0" && existing.device.ip != "::";

                    if new_is_unspecified && existing_is_valid {
                        // Just update last_seen to keep it alive, but don't overwrite valid IP.
                        existing.last_seen = Instant::now();

                        // Still accept better metadata from this signal.
                        if existing.device.name != device.name {
                            existing.device.name = device.name.clone();
                        }

                        if existing.device.device_type == DeviceType::Unknown
                            && device.device_type != DeviceType::Unknown
                        {
                            existing.device.device_type = device.device_type;
                        }
                    } else {
                        *existing = endpoint;
                    }
                } else {
                    tracked.discovered = Some(endpoint);
                }

                // Keep active connected metadata in sync with discovery.
                // This allows peer renames to propagate immediately even while
                // the connection stays up.
                if let Some(connected) = tracked.connected.as_mut() {
                    if connected.device.name != device.name {
                        connected.device.name = device.name.clone();
                    }

                    if device.device_type != DeviceType::Unknown
                        && connected.device.device_type == DeviceType::Unknown
                    {
                        connected.device.device_type = device.device_type;
                    }
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

    /// Sanitize a device name from mDNS: strip control characters and limit length.
    fn sanitize_device_name(raw: &str) -> String {
        let sanitized: String = raw.chars().filter(|c| !c.is_control()).take(64).collect();
        let trimmed = sanitized.trim();
        if trimmed.is_empty() {
            "Unknown".to_string()
        } else {
            trimmed.to_string()
        }
    }

    fn is_blocked_discovery_interface(name: &str) -> bool {
        let name = name.to_ascii_lowercase();
        let blocked_prefixes = [
            "lo",
            "rmnet",
            "r_rmnet",
            "ccmni",
            "rev_rmnet",
            "dummy",
            "p2p",
            "clat",
            "tun",
            "tap",
            "utun",
            "wg",
            "ipsec",
            "vmnet",
            "vbox",
            "docker",
            "br-",
            "veth",
        ];

        blocked_prefixes
            .iter()
            .any(|prefix| name == *prefix || name.starts_with(prefix))
    }

    fn interface_preference_score(name: &str) -> i32 {
        let name = name.to_ascii_lowercase();

        if name.starts_with("aware") {
            80
        } else if name.starts_with("wlan")
            || name.contains("wi-fi")
            || name.contains("wifi")
            || name.starts_with("wl")
        {
            70
        } else if name.starts_with("eth") || name.starts_with("en") {
            60
        } else {
            20
        }
    }

    fn scoped_interface_names(scoped_ip: &ScopedIp) -> Vec<&str> {
        match scoped_ip {
            ScopedIp::V4(v4) => v4
                .interface_ids()
                .iter()
                .map(|id| id.name.as_str())
                .collect(),
            ScopedIp::V6(v6) => vec![v6.scope_id().name.as_str()],
            _ => Vec::new(),
        }
    }

    fn score_scoped_ip(scoped_ip: &ScopedIp) -> Option<(IpAddr, i32, String)> {
        let ip = scoped_ip.to_ip_addr();
        let interface_names = Self::scoped_interface_names(scoped_ip);

        if interface_names
            .iter()
            .any(|name| Self::is_blocked_discovery_interface(name))
        {
            return None;
        }

        let interface_score = interface_names
            .iter()
            .map(|name| Self::interface_preference_score(name))
            .max()
            .unwrap_or(10);

        let ip_score = match ip {
            IpAddr::V4(v4) => {
                if v4.is_unspecified() || v4.is_loopback() || v4.is_link_local() {
                    return None;
                }
                100
            }
            IpAddr::V6(v6) => {
                if v6.is_unspecified() || v6.is_loopback() || v6.is_multicast() {
                    return None;
                }

                if v6.is_unicast_link_local() {
                    if interface_names
                        .iter()
                        .any(|name| name.to_ascii_lowercase().starts_with("aware"))
                    {
                        90
                    } else {
                        return None;
                    }
                } else {
                    80
                }
            }
        };

        let interfaces = if interface_names.is_empty() {
            "unknown".to_string()
        } else {
            interface_names.join(",")
        };

        Some((ip, ip_score + interface_score, interfaces))
    }

    fn select_best_scoped_ip(addresses: &std::collections::HashSet<ScopedIp>) -> Option<IpAddr> {
        let mut candidates: Vec<(IpAddr, i32, String)> =
            addresses.iter().filter_map(Self::score_scoped_ip).collect();

        candidates.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| a.0.to_string().cmp(&b.0.to_string()))
                .then_with(|| a.2.cmp(&b.2))
        });

        if let Some((ip, score, interfaces)) = candidates.first() {
            debug!(
                "Selected discovery address {} from interfaces [{}] with score {}",
                ip, interfaces, score
            );
            Some(*ip)
        } else {
            None
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

        // Get device name from TXT or parse from instance, then sanitize
        let raw_name = info
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
        let device_name = Self::sanitize_device_name(&raw_name);

        let device_type = info
            .txt_properties
            .get("type")
            .map(|v| v.val_str().parse().unwrap_or(DeviceType::Unknown))
            .unwrap_or(DeviceType::Unknown);

        let ip = Self::select_best_scoped_ip(&info.addresses);

        let Some(ip) = ip else {
            warn!(
                "No supported IP address found for device {} from {:?}",
                device_name, info.addresses
            );
            return;
        };

        if matches!(ip, IpAddr::V6(v6) if v6.is_unicast_link_local()) {
            warn!(
                "Discovered link-local IPv6 address for {}. Connectivity may require scope info.",
                device_name
            );
        }

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
        if let Some(old_id) = old_device_to_remove
            && let Err(e) = event_tx.send(DiscoveryEvent::DeviceLost(old_id))
        {
            tracing::warn!("Event dropped: {:?}", e);
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
            if let Err(e) = event_tx.send(event) {
                tracing::warn!("Event dropped: {:?}", e);
            }
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
        let service_fullname = self.service_fullname.read().clone();
        if let Err(e) = self.daemon.unregister(&service_fullname) {
            debug!("Failed to unregister mDNS service: {}", e);
        }

        self.join_thread_handles("shutting down discovery");
        *self.browse_event_tx.write() = None;

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

    pub fn clear_discovered_devices(&self) -> Vec<String> {
        let mut devices = self.discovered_devices.write();
        let ids: Vec<String> = devices.keys().cloned().collect();
        let count = devices.len();
        devices.clear();
        if count > 0 {
            info!("Cleared {} discovered devices", count);
        }
        ids
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

    #[test]
    fn preserves_device_type_when_connected_update_is_unknown() {
        use std::net::Ipv4Addr;

        let mut devices = std::collections::HashMap::new();
        let device_id = "dev-android".to_string();

        let discovered = Device::new(
            device_id.clone(),
            "Pixel".to_string(),
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)),
            44444,
            DeviceType::Android,
        );
        DiscoveryService::upsert_endpoint_locked(
            &mut devices,
            discovered,
            DiscoverySource::Discovered,
        );

        let connected_unknown = Device::new(
            device_id.clone(),
            "Pixel".to_string(),
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)),
            44444,
            DeviceType::Unknown,
        );

        let event = DiscoveryService::upsert_endpoint_locked(
            &mut devices,
            connected_unknown,
            DiscoverySource::Connected,
        );

        match event {
            Some(DiscoveryEvent::DeviceFound(device)) => {
                assert_eq!(device.device_type, DeviceType::Android);
            }
            _ => panic!("expected DeviceFound event"),
        }

        let active = devices
            .get(&device_id)
            .and_then(TrackedDevice::active_device)
            .expect("device should exist");
        assert_eq!(active.device_type, DeviceType::Android);
    }

    #[test]
    fn upgrades_connected_unknown_type_from_discovery_metadata() {
        use std::net::Ipv4Addr;

        let mut devices = std::collections::HashMap::new();
        let device_id = "dev-upgrade".to_string();

        let connected_unknown = Device::new(
            device_id.clone(),
            "Remote".to_string(),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
            44444,
            DeviceType::Unknown,
        );
        DiscoveryService::upsert_endpoint_locked(
            &mut devices,
            connected_unknown,
            DiscoverySource::Connected,
        );

        let discovered_android = Device::new(
            device_id.clone(),
            "Remote".to_string(),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
            44444,
            DeviceType::Android,
        );
        DiscoveryService::upsert_endpoint_locked(
            &mut devices,
            discovered_android,
            DiscoverySource::Discovered,
        );

        let active = devices
            .get(&device_id)
            .and_then(TrackedDevice::active_device)
            .expect("device should exist");
        assert_eq!(active.device_type, DeviceType::Android);
    }

    #[test]
    fn keeps_valid_ip_on_unspecified_discovery_update_but_enriches_type() {
        use std::net::Ipv4Addr;

        let mut devices = std::collections::HashMap::new();
        let device_id = "dev-ble".to_string();

        let discovered_with_valid_ip = Device::new(
            device_id.clone(),
            "Phone".to_string(),
            IpAddr::V4(Ipv4Addr::new(172, 16, 1, 4)),
            44444,
            DeviceType::Unknown,
        );
        DiscoveryService::upsert_endpoint_locked(
            &mut devices,
            discovered_with_valid_ip,
            DiscoverySource::Discovered,
        );

        let discovered_unspecified_with_type = Device::new(
            device_id.clone(),
            "Phone".to_string(),
            IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
            44444,
            DeviceType::Android,
        );
        DiscoveryService::upsert_endpoint_locked(
            &mut devices,
            discovered_unspecified_with_type,
            DiscoverySource::Discovered,
        );

        let tracked = devices.get(&device_id).expect("device should exist");
        let discovered = tracked
            .discovered
            .as_ref()
            .expect("discovered endpoint should exist");

        assert_eq!(discovered.device.ip, "172.16.1.4");
        assert_eq!(discovered.device.device_type, DeviceType::Android);
    }

    #[test]
    fn updates_connected_name_from_discovery_metadata() {
        use std::net::Ipv4Addr;

        let mut devices = std::collections::HashMap::new();
        let device_id = "dev-rename".to_string();

        let connected_old = Device::new(
            device_id.clone(),
            "Office PC".to_string(),
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 77)),
            44444,
            DeviceType::Windows,
        );
        DiscoveryService::upsert_endpoint_locked(
            &mut devices,
            connected_old,
            DiscoverySource::Connected,
        );

        let discovered_new = Device::new(
            device_id.clone(),
            "Office Workstation".to_string(),
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 77)),
            44444,
            DeviceType::Windows,
        );

        let event = DiscoveryService::upsert_endpoint_locked(
            &mut devices,
            discovered_new,
            DiscoverySource::Discovered,
        );

        match event {
            Some(DiscoveryEvent::DeviceFound(device)) => {
                assert_eq!(device.name, "Office Workstation");
            }
            _ => panic!("expected DeviceFound event"),
        }

        let tracked = devices.get(&device_id).expect("device should exist");
        let connected = tracked
            .connected
            .as_ref()
            .expect("connected endpoint should still exist");
        let active = tracked
            .active_device()
            .expect("active endpoint should exist");

        assert_eq!(connected.device.name, "Office Workstation");
        assert_eq!(active.name, "Office Workstation");
    }
}
