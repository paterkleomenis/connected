import Foundation

/// iOS-native Bonjour/mDNS service publisher.
///
/// The Rust `mdns-sd` crate uses raw UDP multicast sockets, which iOS silently
/// blocks for outgoing multicast from third-party apps (requires the
/// `com.apple.developer.networking.multicast` entitlement).  iOS's own
/// `NSNetService` API goes through the system's `mDNSResponder` daemon which
/// has the necessary entitlements, so this works without any special
/// provisioning.
///
/// This runs *alongside* the Rust mDNS — the Rust side still handles
/// **browsing** (receiving), while this handles **publishing** (sending).
///
/// A heartbeat timer re-publishes every 60 seconds because iOS's mDNSResponder
/// can silently drop the service without calling `netServiceDidStop`.
///
/// - Note: Only one instance should exist at a time; held by `ConnectedAppModel`.
final class BonjourPublisher: NSObject {
    private var service: NetService?
    private let queue = DispatchQueue(label: "com.connected.bonjour", qos: .utility)
    private var heartbeatTimer: Timer?

    // Stored for re-publish
    private var lastPort: Int32 = 0
    private var lastDeviceId: String = ""
    private var lastTxt: [String: String] = [:]

    /// Start or update the published Bonjour service.
    /// - Parameters:
    ///   - name: Human-readable device name (e.g. "My iPhone")
    ///   - port: QUIC listen port (from Rust core via `getLocalDevice().port`)
    ///   - deviceId: Unique device UUID (from Rust core)
    ///   - txt: Additional TXT key-value pairs, e.g. `["type": "ios", "version": "1"]`
    func publish(name: String, port: Int32, deviceId: String, txt: [String: String] = [:]) {
        lastPort = port
        lastDeviceId = deviceId
        lastTxt = txt

        queue.async { [weak self] in
            guard let self else { return }

            // Stop previous service
            self.service?.stop()
            self.service?.delegate = nil

            var allTxt = txt
            allTxt["id"] = deviceId
            allTxt["name"] = name

            let svc = NetService(
                domain: "local.",
                type: "_connected._udp.",
                name: name,
                port: port
            )
            svc.delegate = self
            svc.includesPeerToPeer = true

            let txtData = NetService.data(fromTXTRecord: allTxt.mapValues {
                $0.data(using: .utf8) ?? Data()
            })
            svc.setTXTRecord(txtData)
            svc.publish()

            self.service = svc
        }

        startHeartbeat(name: name)
    }

    /// Update only the TXT records (e.g. after a device rename).
    func updateTxt(name: String, txt: [String: String]) {
        queue.async { [weak self] in
            guard let self, let svc = self.service else { return }

            var allTxt = txt
            allTxt["name"] = name

            let txtData = NetService.data(fromTXTRecord: allTxt.mapValues {
                $0.data(using: .utf8) ?? Data()
            })
            svc.setTXTRecord(txtData)
        }
    }

    /// Stop publishing.  Called on shutdown or when the app enters background.
    func stop() {
        stopHeartbeat()
        queue.async { [weak self] in
            guard let self else { return }
            self.service?.stop()
            self.service?.delegate = nil
            self.service = nil
        }
    }

    // MARK: - Heartbeat

    private func startHeartbeat(name: String) {
        stopHeartbeat()
        let name = name
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.heartbeatTimer = Timer.scheduledTimer(withTimeInterval: 60, repeats: true) { [weak self] _ in
                self?.republish(name: name)
            }
        }
    }

    private func stopHeartbeat() {
        DispatchQueue.main.async { [weak self] in
            self?.heartbeatTimer?.invalidate()
            self?.heartbeatTimer = nil
        }
    }

    private func republish(name: String) {
        queue.async { [weak self] in
            guard let self, lastPort != 0, !lastDeviceId.isEmpty else { return }
            self.service?.stop()
            self.service?.delegate = nil

            var allTxt = lastTxt
            allTxt["id"] = lastDeviceId
            allTxt["name"] = name

            let svc = NetService(
                domain: "local.",
                type: "_connected._udp.",
                name: name,
                port: lastPort
            )
            svc.delegate = self
            svc.includesPeerToPeer = true

            let txtData = NetService.data(fromTXTRecord: allTxt.mapValues {
                $0.data(using: .utf8) ?? Data()
            })
            svc.setTXTRecord(txtData)
            svc.publish()

            self.service = svc
        }
    }
}

// MARK: - NetServiceDelegate

extension BonjourPublisher: NetServiceDelegate {
    func netServiceDidPublish(_ sender: NetService) {
        NSLog("Bonjour published: %@", sender.name)
    }

    func netService(_ sender: NetService, didNotPublish errorDict: [String: NSNumber]) {
        NSLog("Bonjour publish failed: %@ — %@", sender.name, errorDict)
    }

    func netServiceDidStop(_ sender: NetService) {
        NSLog("Bonjour stopped: %@", sender.name)
    }
}
