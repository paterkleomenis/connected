import Darwin
import Foundation

@MainActor
protocol BonjourBrowserDelegate: AnyObject {
    func bonjourBrowserDidFindDevice(_ device: DiscoveredDevice)
    func bonjourBrowserDidRemoveDevice(_ deviceId: String)
    func bonjourBrowserDidUpdateStatus(_ status: String)
    func bonjourBrowserDidFail(_ message: String)
}

/// iOS-native Bonjour/mDNS browser.
///
/// Rust `mdns-sd` can be unreliable on iOS because third-party apps do not get
/// normal raw multicast behavior. Browsing through `NetServiceBrowser` goes via
/// the system mDNSResponder, matching the iOS publisher path.
final class BonjourBrowser: NSObject {
    weak var delegate: BonjourBrowserDelegate?

    private let serviceType = "_connected._udp."
    private let domain = "local."
    private var browser: NetServiceBrowser?
    private var servicesByKey: [String: NetService] = [:]
    private var deviceIdsByServiceKey: [String: String] = [:]
    private var localDeviceId: String?
    private var isSearching = false

    func start(localDeviceId: String) {
        self.localDeviceId = localDeviceId
        guard !isSearching else { return }

        let browser = NetServiceBrowser()
        browser.delegate = self
        browser.includesPeerToPeer = true
        self.browser = browser
        self.isSearching = true
        browser.searchForServices(ofType: serviceType, inDomain: domain)
    }

    func restart(localDeviceId: String) {
        stop()
        start(localDeviceId: localDeviceId)
    }

    func stop() {
        isSearching = false
        browser?.stop()
        browser?.delegate = nil
        browser = nil

        for service in servicesByKey.values {
            service.stop()
            service.delegate = nil
        }
        servicesByKey.removeAll()
        deviceIdsByServiceKey.removeAll()
    }

    private func serviceKey(_ service: NetService) -> String {
        "\(service.domain)|\(service.type)|\(service.name)"
    }

    private func handleResolved(_ service: NetService) {
        let txt = Self.txtRecords(from: service)
        guard let deviceId = txt["id"], !deviceId.isEmpty else {
            NSLog("Bonjour browser ignored %@ without Connected TXT id", service.name)
            return
        }
        guard deviceId != localDeviceId else {
            return
        }

        guard let ip = Self.bestAddress(from: service.addresses ?? []) else {
            NSLog("Bonjour browser could not resolve usable address for %@", service.name)
            return
        }

        let name = txt["name"].flatMap { $0.isEmpty ? nil : $0 } ?? service.name
        let deviceType = txt["type"].flatMap { $0.isEmpty ? nil : $0 } ?? "unknown"
        let port = UInt16(clamping: service.port)
        let device = DiscoveredDevice(
            id: deviceId,
            name: name,
            ip: ip,
            port: port,
            deviceType: deviceType
        )

        deviceIdsByServiceKey[serviceKey(service)] = deviceId

        do {
            try injectProximityDevice(
                deviceId: device.id,
                deviceName: device.name,
                deviceType: device.deviceType,
                ip: device.ip,
                port: device.port
            )
        } catch {
            Task { @MainActor [weak delegate] in
                delegate?.bonjourBrowserDidFail(
                    "Bonjour device injection failed for \(name): \(error.localizedDescription)"
                )
            }
            return
        }

        Task { @MainActor [weak delegate] in
            delegate?.bonjourBrowserDidFindDevice(device)
            delegate?.bonjourBrowserDidUpdateStatus("Found \(name) on local network.")
        }
    }

    private static func txtRecords(from service: NetService) -> [String: String] {
        guard let data = service.txtRecordData() else { return [:] }
        let raw = NetService.dictionary(fromTXTRecord: data)
        var records: [String: String] = [:]

        for (key, value) in raw {
            records[key] = String(data: value, encoding: .utf8)
        }
        return records
    }

    private static func bestAddress(from addresses: [Data]) -> String? {
        let parsed = addresses.compactMap(Self.parseAddress)

        if let ipv4 = parsed.first(where: { $0.family == AF_INET && !$0.ip.hasPrefix("169.254.") }) {
            return ipv4.ip
        }
        if let ipv6 = parsed.first(where: { $0.family == AF_INET6 && !$0.ip.lowercased().hasPrefix("fe80:") }) {
            return ipv6.ip
        }
        if let linkLocal = parsed.first(where: { $0.family == AF_INET6 }) {
            return linkLocal.ip
        }
        return parsed.first?.ip
    }

    private static func parseAddress(_ data: Data) -> (ip: String, family: Int32)? {
        data.withUnsafeBytes { rawBuffer -> (String, Int32)? in
            guard let baseAddress = rawBuffer.baseAddress else { return nil }
            let sockaddrPointer = baseAddress.assumingMemoryBound(to: sockaddr.self)
            let family = Int32(sockaddrPointer.pointee.sa_family)
            let length: socklen_t

            #if os(iOS)
            length = socklen_t(sockaddrPointer.pointee.sa_len)
            #else
            length = family == AF_INET6
                ? socklen_t(MemoryLayout<sockaddr_in6>.size)
                : socklen_t(MemoryLayout<sockaddr_in>.size)
            #endif

            var host = [CChar](repeating: 0, count: Int(NI_MAXHOST))
            let result = getnameinfo(
                sockaddrPointer,
                length,
                &host,
                socklen_t(host.count),
                nil,
                0,
                NI_NUMERICHOST
            )

            guard result == 0 else { return nil }

            var ip = String(cString: host)
            if let scopeIndex = ip.firstIndex(of: "%") {
                ip = String(ip[..<scopeIndex])
            }
            return (ip, family)
        }
    }
}

extension BonjourBrowser: NetServiceBrowserDelegate {
    func netServiceBrowserWillSearch(_ browser: NetServiceBrowser) {
        Task { @MainActor [weak delegate] in
            delegate?.bonjourBrowserDidUpdateStatus("Searching local network.")
        }
    }

    func netServiceBrowserDidStopSearch(_ browser: NetServiceBrowser) {
        Task { @MainActor [weak delegate] in
            delegate?.bonjourBrowserDidUpdateStatus("Stopped local network search.")
        }
    }

    func netServiceBrowser(
        _ browser: NetServiceBrowser,
        didNotSearch errorDict: [String: NSNumber]
    ) {
        isSearching = false
        NSLog("Bonjour search failed: %@", "\(errorDict)")
    }

    func netServiceBrowser(
        _ browser: NetServiceBrowser,
        didFind service: NetService,
        moreComing: Bool
    ) {
        let txt = Self.txtRecords(from: service)
        if let deviceId = txt["id"], deviceId == localDeviceId {
            return
        }
        let key = serviceKey(service)
        servicesByKey[key] = service
        service.delegate = self
        service.resolve(withTimeout: 5)
    }

    func netServiceBrowser(
        _ browser: NetServiceBrowser,
        didRemove service: NetService,
        moreComing: Bool
    ) {
        let key = serviceKey(service)
        if let deviceId = deviceIdsByServiceKey[key] {
            servicesByKey[key]?.stop()
            servicesByKey[key]?.delegate = nil
            servicesByKey.removeValue(forKey: key)
            deviceIdsByServiceKey.removeValue(forKey: key)
            Task { @MainActor [weak delegate] in
                delegate?.bonjourBrowserDidRemoveDevice(deviceId)
            }
        }
    }
}

extension BonjourBrowser: NetServiceDelegate {
    func netServiceDidResolveAddress(_ sender: NetService) {
        handleResolved(sender)
    }

    func netService(_ sender: NetService, didNotResolve errorDict: [String: NSNumber]) {
        NSLog("Bonjour resolve failed for %@: %@", sender.name, "\(errorDict)")
        let key = serviceKey(sender)
        if let deviceId = deviceIdsByServiceKey[key] {
            servicesByKey[key]?.stop()
            servicesByKey[key]?.delegate = nil
            servicesByKey.removeValue(forKey: key)
            deviceIdsByServiceKey.removeValue(forKey: key)
            Task { @MainActor [weak delegate] in
                delegate?.bonjourBrowserDidRemoveDevice(deviceId)
            }
        }
    }

    func netService(_ sender: NetService, didUpdateTXTRecord data: Data) {
        handleResolved(sender)
    }
}
