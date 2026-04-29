import Foundation
import SwiftUI
#if canImport(UIKit)
import UIKit
#endif
#if canImport(Contacts)
import Contacts
#endif
#if canImport(MediaPlayer)
import MediaPlayer
#endif
#if canImport(UserNotifications)
import UserNotifications
#endif

@MainActor
final class ConnectedAppModel: ObservableObject {
    enum ThemeMode: String, CaseIterable, Identifiable {
        case system
        case light
        case dark

        var id: String { rawValue }
    }

    enum RootTab: Hashable {
        case devices
        case settings
    }

    struct PairingPrompt: Identifiable, Equatable {
        var id: String { deviceId }
        let deviceName: String
        let fingerprint: String
        let deviceId: String
    }

    struct TransferPrompt: Identifiable, Equatable {
        var id: String { transferId }
        let transferId: String
        let filename: String
        let fileSize: UInt64
        let fromDevice: String
    }

    struct BrowserDownloadProgressState: Equatable {
        let currentFile: String
        let bytesDownloaded: UInt64
        let totalBytes: UInt64
        let isFolder: Bool

        var fractionCompleted: Double {
            guard totalBytes > 0 else { return 0 }
            return Double(bytesDownloaded) / Double(totalBytes)
        }
    }

    @Published private(set) var devices: [DiscoveredDevice] = []
    @Published private(set) var trustedDeviceIds: Set<String> = []
    @Published private(set) var pendingPairing: Set<String> = []
    @Published var selectedDeviceId: String?

    @Published var localDevice: DiscoveredDevice?
    @Published var localFingerprint: String = ""

    @Published var transferStatus: String = "Idle"
    @Published var activeTransferId: String?
    @Published var pairingRequest: PairingPrompt?
    @Published var transferRequest: TransferPrompt?

    @Published var clipboardContent: String = ""
    @Published var remoteFiles: [FfiFsEntry] = []
    @Published var currentRemotePath: String = "/"
    @Published var browsingDevice: DiscoveredDevice?
    @Published var browserDownloadProgress: BrowserDownloadProgressState?
    @Published private(set) var thumbnailDataByPath: [String: Data] = [:]
    @Published private(set) var requestedThumbnails: Set<String> = []

    @Published var contacts: [FfiContact] = []
    @Published var conversations: [FfiConversation] = []
    @Published var messages: [FfiSmsMessage] = []
    @Published var callLog: [FfiCallLogEntry] = []
    @Published var activeCall: FfiActiveCall?
    @Published var selectedConversationThreadId: String?

    @Published var currentMediaState = MediaState(title: nil, artist: nil, album: nil, playing: false)
    @Published var mediaTitleDraft = ""
    @Published var mediaArtistDraft = ""
    @Published var mediaAlbumDraft = ""
    @Published var mediaPlayingDraft = false

    @Published var isClipboardSyncEnabled = false
    @Published var isMediaControlEnabled = true
    @Published var isTelephonyEnabled = true

    @Published var themeMode: ThemeMode = .system
    @Published var selectedRootTab: RootTab = .devices

    @Published var pairingModeEnabled = true
    @Published var isInitialized = false
    @Published var isDiscoveryActive = false
    @Published var shouldHighlightPasteAndShare = false
    @Published var infoMessage: String?
    @Published var lastErrorMessage: String?
    @Published private(set) var pendingShareURLs: [URL] = []
    @Published private(set) var downloadDirectoryDisplayName = "Downloads"
    @Published private(set) var sharedDirectoryDisplayName = "Shared"
    @Published private(set) var contactsPermissionGranted = false
    @Published private(set) var mediaLibraryPermissionGranted = false
    @Published private(set) var notificationsPermissionGranted = false

    @Published var deviceName: String

    let storageRoot: URL
    let defaultDownloadRoot: URL
    @Published private(set) var downloadRoot: URL
    let defaultSharedRoot: URL
    @Published private(set) var sharedRoot: URL

    private let defaults = UserDefaults.standard
    private let defaultsDeviceNameKey = "connected.ios.deviceName"
    private let defaultsThemeModeKey = "connected.ios.themeMode"
    private let defaultsClipboardSyncKey = "connected.ios.clipboardSync"
    private let defaultsMediaControlKey = "connected.ios.mediaControl"
    private let defaultsTelephonyEnabledKey = "connected.ios.telephonyEnabled"
    private let defaultsPairingModeKey = "connected.ios.pairingMode"
    private let defaultsDownloadFolderBookmarkKey = "connected.ios.downloadFolderBookmark"
    private let defaultsSharedFolderBookmarkKey = "connected.ios.sharedFolderBookmark"
    private let clipboardNotificationIdentifier = "connected.clipboard.share"
    private let clipboardReceivedNotificationIdentifier = "connected.clipboard.received"
    private let clipboardNotificationCategoryIdentifier = "connected.clipboard.category"
    private let clipboardNotificationActionIdentifier = "connected.clipboard.share.action"
    private let transferNotificationIdentifier = "connected.transfer.status"

    private let discoveryBridge: DiscoveryBridge
    private let transferBridge: TransferBridge
    private let clipboardBridge: ClipboardBridge
    private let pairingBridge: PairingBridge
    private let unpairBridge: UnpairBridge
    private let mediaBridge: MediaBridge
    private let telephonyBridge: TelephonyBridge
    private let browserDownloadBridge: BrowserDownloadBridge
#if canImport(UserNotifications)
    private let notificationBridge: ClipboardNotificationBridge
#endif
    private var fsProvider: IOSFilesystemProvider

    private let fallbackDeviceName = "iPhone"
    private var lastRemoteClipboard = ""
    private var securityScopedDownloadRootURL: URL?
    private var securityScopedSharedRootURL: URL?
    private var latestLocalMediaState: MediaState?
    private var lastBroadcastLocalMediaState: MediaState?
    private var transferNotificationNeedsAcknowledgment = false
    private var lastTransferNotificationProgressBucket: Int?
#if canImport(MediaPlayer)
    private var mediaPlaybackObservers: [NSObjectProtocol] = []
#endif
#if canImport(UIKit)
    private var clipboardObserver: NSObjectProtocol?
    private var backgroundTaskIdentifier: UIBackgroundTaskIdentifier = .invalid
#endif

    init() {
        let fileManager = FileManager.default
        let appSupport = fileManager.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        let docs = fileManager.urls(for: .documentDirectory, in: .userDomainMask).first!

        storageRoot = appSupport.appendingPathComponent("Connected", isDirectory: true)
        defaultDownloadRoot = docs.appendingPathComponent("Downloads", isDirectory: true)
        downloadRoot = defaultDownloadRoot
        defaultSharedRoot = docs.appendingPathComponent("Shared", isDirectory: true)
        sharedRoot = defaultSharedRoot

        let persistedName = defaults.string(forKey: defaultsDeviceNameKey)

#if canImport(UIKit)
        let defaultDeviceName = UIDevice.current.name
#else
        let defaultDeviceName = fallbackDeviceName
#endif

        deviceName = (persistedName?.isEmpty == false ? (persistedName ?? defaultDeviceName) : defaultDeviceName)

        if let storedTheme = defaults.string(forKey: defaultsThemeModeKey),
           let mode = ThemeMode(rawValue: storedTheme) {
            themeMode = mode
        }

        isClipboardSyncEnabled = defaults.object(forKey: defaultsClipboardSyncKey) as? Bool ?? false
        isMediaControlEnabled = defaults.object(forKey: defaultsMediaControlKey) as? Bool ?? true
        isTelephonyEnabled = defaults.object(forKey: defaultsTelephonyEnabledKey) as? Bool ?? true
        pairingModeEnabled = defaults.object(forKey: defaultsPairingModeKey) as? Bool ?? true

        var resolvedDownloadRoot = defaultDownloadRoot
        var resolvedSharedRoot = defaultSharedRoot

        try? fileManager.createDirectory(at: storageRoot, withIntermediateDirectories: true)
        try? fileManager.createDirectory(at: defaultDownloadRoot, withIntermediateDirectories: true)
        try? fileManager.createDirectory(at: defaultSharedRoot, withIntermediateDirectories: true)

        if let restored = Self.resolveFolderBookmark(
            fileManager: fileManager,
            defaults: defaults,
            defaultsKey: defaultsDownloadFolderBookmarkKey
        ) {
            resolvedDownloadRoot = restored
        }

        if let restored = Self.resolveFolderBookmark(
            fileManager: fileManager,
            defaults: defaults,
            defaultsKey: defaultsSharedFolderBookmarkKey
        ) {
            resolvedSharedRoot = restored
        }

        downloadRoot = resolvedDownloadRoot
        sharedRoot = resolvedSharedRoot

        downloadDirectoryDisplayName = resolvedDownloadRoot.lastPathComponent
        sharedDirectoryDisplayName = resolvedSharedRoot.lastPathComponent

        fsProvider = IOSFilesystemProvider(rootURL: resolvedSharedRoot)

        discoveryBridge = DiscoveryBridge()
        transferBridge = TransferBridge()
        clipboardBridge = ClipboardBridge()
        pairingBridge = PairingBridge()
        unpairBridge = UnpairBridge()
        mediaBridge = MediaBridge()
        telephonyBridge = TelephonyBridge()
        browserDownloadBridge = BrowserDownloadBridge()
#if canImport(UserNotifications)
        notificationBridge = ClipboardNotificationBridge()
#endif

        discoveryBridge.app = self
        transferBridge.app = self
        clipboardBridge.app = self
        pairingBridge.app = self
        unpairBridge.app = self
        mediaBridge.app = self
        telephonyBridge.app = self
        browserDownloadBridge.app = self
#if canImport(UserNotifications)
        notificationBridge.app = self
        UNUserNotificationCenter.current().delegate = notificationBridge
        registerClipboardNotificationCategory()
        refreshNotificationAuthorizationStatus()
#endif

        if resolvedDownloadRoot.path != defaultDownloadRoot.path {
            _ = beginPersistentDownloadRootAccess(for: resolvedDownloadRoot)
        }

        if resolvedSharedRoot.path != defaultSharedRoot.path {
            _ = beginPersistentSharedRootAccess(for: resolvedSharedRoot)
        }

#if canImport(UIKit)
        contactsPermissionGranted = CNContactStore.authorizationStatus(for: .contacts) == .authorized
        startClipboardMonitorIfNeeded()
#endif
#if canImport(MediaPlayer)
        mediaLibraryPermissionGranted = MPMediaLibrary.authorizationStatus() == .authorized
#endif

        mediaTitleDraft = currentMediaState.title ?? ""
        mediaArtistDraft = currentMediaState.artist ?? ""
        mediaAlbumDraft = currentMediaState.album ?? ""
        mediaPlayingDraft = currentMediaState.playing

        if isMediaControlEnabled {
            startLocalMediaObserverIfNeeded()
        }
    }

    nonisolated deinit {
#if canImport(UIKit)
        if let observer = clipboardObserver {
            NotificationCenter.default.removeObserver(observer)
        }
#endif
#if canImport(MediaPlayer)
        for observer in mediaPlaybackObservers {
            NotificationCenter.default.removeObserver(observer)
        }
        MPMusicPlayerController.systemMusicPlayer.endGeneratingPlaybackNotifications()
#endif
        if let scopedURL = securityScopedDownloadRootURL {
            scopedURL.stopAccessingSecurityScopedResource()
        }
        if let scopedURL = securityScopedSharedRootURL {
            scopedURL.stopAccessingSecurityScopedResource()
        }
    }

    var activeDevice: DiscoveredDevice? {
        if let id = selectedDeviceId {
            return devices.first(where: { $0.id == id })
        }
        return nil
    }

    var hasTrustedDevices: Bool {
        devices.contains { trustedDeviceIds.contains($0.id) }
    }

    func initializeIfNeeded() {
        guard !isInitialized else { return }

        let currentName = deviceName
        let storagePath = storageRoot.path
        let downloadPath = downloadRoot.path
        let pairingMode = pairingModeEnabled
        let discovery = discoveryBridge
        let transfer = transferBridge
        let clipboard = clipboardBridge
        let pairing = pairingBridge
        let unpair = unpairBridge
        let media = mediaBridge
        let telephony = telephonyBridge
        let fs = fsProvider

        runInBackground { [weak self] in
            guard let self else { return }

            do {
                do {
                    try initialize(deviceName: currentName, deviceType: "ios", bindPort: 0, storagePath: storagePath)
                } catch let error as ConnectedFfiError {
                    switch error {
                    case .InitializationError:
                        break
                    default:
                        throw error
                    }
                }

                try registerFilesystemProvider(callback: fs)
                registerTransferCallback(callback: transfer)
                registerClipboardReceiver(callback: clipboard)
                registerPairingCallback(callback: pairing)
                registerUnpairCallback(callback: unpair)
                registerMediaControlCallback(callback: media)
                registerTelephonyCallback(callback: telephony)
                try setDownloadDirectory(path: downloadPath)
                try setPairingMode(enabled: pairingMode)

                do {
                    try startDiscovery(callback: discovery)
                } catch {
                    let message = String(describing: error).lowercased()
                    if !message.contains("already") {
                        throw error
                    }
                }

                let discovered = (try? getDiscoveredDevices()) ?? []
                let localDevice = try? getLocalDevice()
                let localFingerprint = (try? getLocalFingerprint()) ?? ""

                Task { @MainActor [weak self] in
                    guard let self else { return }
                    self.isInitialized = true
                    self.isDiscoveryActive = true
                    self.pairingModeEnabled = pairingMode
                    self.localDevice = localDevice
                    self.localFingerprint = localFingerprint
                    self.mergeDevices(discovered)
                    self.infoMessage = "Connected core initialized"
                    self.lastErrorMessage = nil
                    if self.isMediaControlEnabled {
                        self.startLocalMediaObserverIfNeeded()
                        self.refreshLocalMediaStateAndBroadcast()
                    }
                }
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Initialization failed: \(error.localizedDescription)"
                }
            }
        }
    }

    func handleAppBecameActive() {
        initializeIfNeeded()
        endBackgroundNetworkingTask()
        refreshNotificationAuthorizationStatus()
        acknowledgeTransferNotificationIfNeeded()

        if !isDiscoveryActive {
            setDiscoveryActive(true)
        }
    }

    func handleAppEnteredBackground() {
        beginBackgroundNetworkingTask()
        scheduleClipboardShareNotificationIfNeeded()
        if isInitialized {
            infoMessage = "Connected will keep networking while iOS allows background activity."
        }
    }

    func setDeviceName(_ newName: String) {
        let trimmed = newName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        deviceName = trimmed
        defaults.set(trimmed, forKey: defaultsDeviceNameKey)
        infoMessage = "Saved device name. Reopen the app if peers still show the old name."
    }

    func updatePairingMode(enabled: Bool) {
        runInBackground { [weak self] in
            do {
                try setPairingMode(enabled: enabled)
                Task { @MainActor [weak self] in
                    guard let self else { return }
                    self.pairingModeEnabled = enabled
                    self.defaults.set(enabled, forKey: self.defaultsPairingModeKey)
                }
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Failed to update pairing mode: \(error.localizedDescription)"
                }
            }
        }
    }

    func refreshDiscoveryNow() {
        runInBackground { [weak self] in
            do {
                try refreshDiscovery()
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Discovery refresh failed: \(error.localizedDescription)"
                }
            }
        }
    }

    func setDiscoveryActive(_ active: Bool) {
        guard isInitialized else { return }
        guard active != isDiscoveryActive else { return }

        if active {
            let discovery = discoveryBridge
            runInBackground { [weak self] in
                do {
                    try startDiscovery(callback: discovery)
                    Task { @MainActor [weak self] in
                        self?.isDiscoveryActive = true
                    }
                } catch {
                    let message = String(describing: error).lowercased()
                    if message.contains("already") {
                        Task { @MainActor [weak self] in
                            self?.isDiscoveryActive = true
                        }
                        return
                    }

                    Task { @MainActor [weak self] in
                        self?.lastErrorMessage = "Failed to start discovery: \(error.localizedDescription)"
                    }
                }
            }
        } else {
            runInBackground { [weak self] in
                stopDiscovery()
                Task { @MainActor [weak self] in
                    self?.isDiscoveryActive = false
                }
            }
        }
    }

    func isTrusted(_ device: DiscoveredDevice) -> Bool {
        trustedDeviceIds.contains(device.id)
    }

    func isPending(_ device: DiscoveredDevice) -> Bool {
        pendingPairing.contains(device.id)
    }

    func requestPairing(with device: DiscoveredDevice) {
        pendingPairing.insert(device.id)
        lastErrorMessage = nil

        runInBackground { [weak self] in
            do {
                try pairDevice(targetIp: device.ip, targetPort: device.port)
            } catch {
                Task { @MainActor [weak self] in
                    self?.pendingPairing.remove(device.id)
                    self?.lastErrorMessage = "Pairing failed: \(error.localizedDescription)"
                }
            }
        }
    }

    func trustCurrentPairingRequest() {
        guard let request = pairingRequest else { return }
        let device = devices.first(where: { $0.id == request.deviceId })

        runInBackground { [weak self] in
            do {
                if request.fingerprint != "Verified (You initiated)" {
                    try trustDevice(
                        fingerprint: request.fingerprint,
                        deviceId: request.deviceId,
                        name: request.deviceName
                    )
                }

                if let device {
                    try sendTrustConfirmation(targetIp: device.ip, targetPort: device.port)
                }

                Task { @MainActor [weak self] in
                    guard let self else { return }
                    self.trustedDeviceIds.insert(request.deviceId)
                    self.pendingPairing.remove(request.deviceId)
                    self.pairingRequest = nil
                    self.infoMessage = "Trusted \(request.deviceName)"
                }
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Trust failed: \(error.localizedDescription)"
                }
            }
        }
    }

    func rejectCurrentPairingRequest() {
        guard let request = pairingRequest else { return }

        runInBackground { [weak self] in
            do {
                try rejectPairing(deviceId: request.deviceId)
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Reject failed: \(error.localizedDescription)"
                }
            }

            Task { @MainActor [weak self] in
                self?.pendingPairing.remove(request.deviceId)
                self?.pairingRequest = nil
            }
        }
    }

    func unpairDevice(_ device: DiscoveredDevice) {
        runInBackground { [weak self] in
            do {
                try unpairDeviceById(deviceId: device.id)
                Task { @MainActor [weak self] in
                    self?.trustedDeviceIds.remove(device.id)
                    self?.pendingPairing.remove(device.id)
                }
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Unpair failed: \(error.localizedDescription)"
                }
            }
        }
    }

    func forgetDevice(_ device: DiscoveredDevice) {
        runInBackground { [weak self] in
            do {
                try forgetDeviceById(deviceId: device.id)
                Task { @MainActor [weak self] in
                    self?.trustedDeviceIds.remove(device.id)
                    self?.pendingPairing.remove(device.id)
                }
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Forget failed: \(error.localizedDescription)"
                }
            }
        }
    }

    func sendFileToDevice(at fileURL: URL, to device: DiscoveredDevice) {
        runInBackground { [weak self] in
            let didOpen = fileURL.startAccessingSecurityScopedResource()
            defer {
                if didOpen {
                    fileURL.stopAccessingSecurityScopedResource()
                }
            }

            do {
                let transferId = try sendFile(
                    targetIp: device.ip,
                    targetPort: device.port,
                    filePath: fileURL.path
                )

                Task { @MainActor [weak self] in
                    self?.activeTransferId = transferId
                    self?.transferStatus = "Sending \(fileURL.lastPathComponent)"
                }
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Send file failed: \(error.localizedDescription)"
                }
            }
        }
    }

    func handleIncomingShareURL(_ url: URL) {
        let trustedTargets = devices.filter { trustedDeviceIds.contains($0.id) }
        guard trustedTargets.count == 1, let device = trustedTargets.first else {
            pendingShareURLs.append(url)
            infoMessage = "Share queued. Use a device menu to send it."
            return
        }
        sendFileToDevice(at: url, to: device)
    }

    func sendClipboardToSelected(text: String) {
        guard let device = activeDevice else { return }
        sendClipboardText(text, to: device)
    }

    func sendClipboardText(_ text: String, to device: DiscoveredDevice) {
        guard !text.isEmpty else { return }
        let clipboard = clipboardBridge

        runInBackground { [weak self] in
            do {
                try sendClipboard(
                    targetIp: device.ip,
                    targetPort: device.port,
                    text: text,
                    callback: clipboard
                )
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Clipboard send failed: \(error.localizedDescription)"
                }
            }
        }
    }

    func sendMediaCommand(_ command: MediaCommand, to device: DiscoveredDevice) {
        runInBackground { [weak self] in
            do {
                try Connected.sendMediaCommand(targetIp: device.ip, targetPort: device.port, command: command)
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Media command failed: \(error.localizedDescription)"
                }
            }
        }
    }

    func requestContacts(from device: DiscoveredDevice) {
        contacts = []
        runInBackground { [weak self] in
            do {
                try requestContactsSync(targetIp: device.ip, targetPort: device.port)
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Contacts sync request failed: \(error.localizedDescription)"
                }
            }
        }
    }

    func requestConversations(from device: DiscoveredDevice) {
        conversations = []
        messages = []
        selectedConversationThreadId = nil
        runInBackground { [weak self] in
            do {
                try requestConversationsSync(targetIp: device.ip, targetPort: device.port)
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Conversations sync request failed: \(error.localizedDescription)"
                }
            }
        }
    }

    func requestCallLog(from device: DiscoveredDevice, limit: UInt32 = 100, reset: Bool = true) {
        if reset {
            callLog = []
        }
        runInBackground { [weak self] in
            do {
                try Connected.requestCallLog(targetIp: device.ip, targetPort: device.port, limit: limit)
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Call log request failed: \(error.localizedDescription)"
                }
            }
        }
    }

    func acceptCurrentTransferRequest() {
        guard let request = transferRequest else { return }
        runInBackground { [weak self] in
            do {
                try acceptFileTransfer(transferId: request.transferId)
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Accept transfer failed: \(error.localizedDescription)"
                }
            }

            Task { @MainActor [weak self] in
                self?.transferRequest = nil
            }
        }
    }

    func rejectCurrentTransferRequest() {
        guard let request = transferRequest else { return }
        runInBackground { [weak self] in
            do {
                try rejectFileTransfer(transferId: request.transferId)
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Reject transfer failed: \(error.localizedDescription)"
                }
            }

            Task { @MainActor [weak self] in
                self?.transferRequest = nil
            }
        }
    }

    func cancelActiveTransfer() {
        guard let transferId = activeTransferId else { return }
        runInBackground { [weak self] in
            do {
                try cancelFileTransfer(transferId: transferId)
                Task { @MainActor [weak self] in
                    self?.transferStatus = "Transfer cancelled"
                    self?.activeTransferId = nil
                }
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Cancel transfer failed: \(error.localizedDescription)"
                }
            }
        }
    }

    func browse(device: DiscoveredDevice, path: String = "/") {
        browsingDevice = device
        currentRemotePath = path

        runInBackground { [weak self] in
            do {
                let entries = try requestListDir(targetIp: device.ip, targetPort: device.port, path: path)
                Task { @MainActor [weak self] in
                    self?.remoteFiles = entries
                    self?.currentRemotePath = path
                    self?.lastErrorMessage = nil
                }
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Browse failed: \(error.localizedDescription)"
                }
            }
        }
    }

    func browseRootOfActiveDevice() {
        guard let device = activeDevice else { return }
        browse(device: device, path: "/")
    }

    func browseParentDirectory() {
        guard let device = browsingDevice ?? activeDevice else { return }
        guard currentRemotePath != "/" else {
            browse(device: device, path: "/")
            return
        }

        var parts = currentRemotePath.split(separator: "/").map(String.init)
        _ = parts.popLast()
        let parent = parts.isEmpty ? "/" : "/" + parts.joined(separator: "/")
        browse(device: device, path: parent)
    }

    func openRemoteEntry(_ entry: FfiFsEntry) {
        guard let device = browsingDevice ?? activeDevice else { return }
        if entry.entryType == .directory {
            browse(device: device, path: entry.path)
        }
    }

    func downloadRemoteEntry(_ entry: FfiFsEntry) {
        guard let device = browsingDevice ?? activeDevice else { return }

        let localPath = downloadRoot.appendingPathComponent(entry.name).path
        browserDownloadProgress = BrowserDownloadProgressState(
            currentFile: entry.name,
            bytesDownloaded: 0,
            totalBytes: max(entry.size, 1),
            isFolder: entry.entryType == .directory
        )

        let callback = browserDownloadBridge

        runInBackground {
            if entry.entryType == .directory {
                requestDownloadFolder(
                    targetIp: device.ip,
                    targetPort: device.port,
                    remotePath: entry.path,
                    localPath: localPath,
                    callback: callback
                )
            } else {
                requestDownloadFileWithProgress(
                    targetIp: device.ip,
                    targetPort: device.port,
                    remotePath: entry.path,
                    localPath: localPath,
                    callback: callback
                )
            }
        }
    }

    func requestContactsForActiveDevice() {
        guard let device = activeDevice else { return }
        requestContacts(from: device)
    }

    func requestConversationsForActiveDevice() {
        guard let device = activeDevice else { return }
        requestConversations(from: device)
    }

    func requestCallLogForActiveDevice(limit: UInt32 = 100) {
        guard let device = activeDevice else { return }
        requestCallLog(from: device, limit: limit)
    }

    func sendMediaCommandToActiveDevice(_ command: MediaCommand) {
        guard let device = activeDevice else { return }
        sendMediaCommand(command, to: device)
    }

    func setThemeMode(_ mode: ThemeMode) {
        themeMode = mode
        defaults.set(mode.rawValue, forKey: defaultsThemeModeKey)
    }

    func setClipboardSyncEnabled(_ enabled: Bool) {
        isClipboardSyncEnabled = enabled
        defaults.set(enabled, forKey: defaultsClipboardSyncKey)
        if enabled {
            refreshNotificationAuthorizationStatus()
            infoMessage = "Clipboard Sync will share new clipboard changes while Connected is active."
        } else {
            cancelClipboardShareNotification()
        }
    }

    func setMediaControlEnabled(_ enabled: Bool) {
        isMediaControlEnabled = enabled
        defaults.set(enabled, forKey: defaultsMediaControlKey)
        if enabled {
            startLocalMediaObserverIfNeeded()
            refreshLocalMediaStateAndBroadcast()
        } else {
            stopLocalMediaObserver()
            latestLocalMediaState = nil
            lastBroadcastLocalMediaState = nil
        }
    }

    func setTelephonyEnabled(_ enabled: Bool) {
        isTelephonyEnabled = enabled
        defaults.set(enabled, forKey: defaultsTelephonyEnabledKey)
    }

    func requestContactsPermission() {
#if canImport(Contacts)
        let store = CNContactStore()
        store.requestAccess(for: .contacts) { granted, _ in
            Task { @MainActor [weak self] in
                self?.contactsPermissionGranted = granted
                if !granted {
                    self?.lastErrorMessage = "Contacts permission denied."
                }
            }
        }
#else
        contactsPermissionGranted = false
#endif
    }

    func requestMediaLibraryPermission() {
#if canImport(MediaPlayer)
        MPMediaLibrary.requestAuthorization { [weak self] status in
            Task { @MainActor [weak self] in
                guard let self else { return }
                self.mediaLibraryPermissionGranted = status == .authorized
                if status == .authorized {
                    self.startLocalMediaObserverIfNeeded()
                    self.refreshLocalMediaStateAndBroadcast()
                } else {
                    self.infoMessage = "iOS can only report Apple Music playback after Music access is granted."
                }
            }
        }
#else
        mediaLibraryPermissionGranted = false
#endif
    }

    func requestClipboardNotificationPermission() {
#if canImport(UserNotifications)
        registerClipboardNotificationCategory()
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) { [weak self] granted, error in
            Task { @MainActor [weak self] in
                guard let self else { return }
                self.notificationsPermissionGranted = granted
                if granted {
                    self.infoMessage = "Background clipboard notification enabled."
                } else if let error {
                    self.lastErrorMessage = "Notification permission failed: \(error.localizedDescription)"
                } else {
                    self.lastErrorMessage = "Notifications permission denied."
                }
            }
        }
#else
        notificationsPermissionGranted = false
#endif
    }

    func setDownloadDirectoryFromURL(_ url: URL) {
        let standardized = url.standardizedFileURL
        let started = standardized.startAccessingSecurityScopedResource()

#if os(macOS)
        let bookmarkCreationOptions: URL.BookmarkCreationOptions = [.withSecurityScope]
#else
        let bookmarkCreationOptions: URL.BookmarkCreationOptions = [.minimalBookmark]
#endif

        do {
            let bookmark = try standardized.bookmarkData(
                options: bookmarkCreationOptions,
                includingResourceValuesForKeys: nil,
                relativeTo: nil
            )
            defaults.set(bookmark, forKey: defaultsDownloadFolderBookmarkKey)

            try FileManager.default.createDirectory(at: standardized, withIntermediateDirectories: true)

            if started {
                standardized.stopAccessingSecurityScopedResource()
            }

            _ = beginPersistentDownloadRootAccess(for: standardized)

            downloadRoot = standardized
            downloadDirectoryDisplayName = standardized.lastPathComponent

            guard isInitialized else {
                infoMessage = "Receive folder updated. It will apply after core starts."
                return
            }

            runInBackground { [weak self] in
                do {
                    try setDownloadDirectory(path: standardized.path)
                    Task { @MainActor [weak self] in
                        self?.infoMessage = "Receive folder updated"
                    }
                } catch {
                    Task { @MainActor [weak self] in
                        self?.lastErrorMessage = "Failed to update receive folder: \(error.localizedDescription)"
                    }
                }
            }
        } catch {
            if started {
                standardized.stopAccessingSecurityScopedResource()
            }
            lastErrorMessage = "Invalid receive folder: \(error.localizedDescription)"
        }
    }

    func resetDownloadDirectoryToDefault() {
        defaults.removeObject(forKey: defaultsDownloadFolderBookmarkKey)
        endPersistentDownloadRootAccess()

        do {
            try FileManager.default.createDirectory(at: defaultDownloadRoot, withIntermediateDirectories: true)
            downloadRoot = defaultDownloadRoot
            downloadDirectoryDisplayName = defaultDownloadRoot.lastPathComponent
        } catch {
            lastErrorMessage = "Failed to reset receive folder: \(error.localizedDescription)"
            return
        }

        guard isInitialized else {
            infoMessage = "Receive folder reset to default."
            return
        }

        let path = defaultDownloadRoot.path
        runInBackground { [weak self] in
            do {
                try setDownloadDirectory(path: path)
                Task { @MainActor [weak self] in
                    self?.infoMessage = "Receive folder reset to default"
                }
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Failed to reset receive folder: \(error.localizedDescription)"
                }
            }
        }
    }

    func setSharedDirectoryFromURL(_ url: URL) {
        let standardized = url.standardizedFileURL
        let started = standardized.startAccessingSecurityScopedResource()

#if os(macOS)
        let bookmarkCreationOptions: URL.BookmarkCreationOptions = [.withSecurityScope]
#else
        let bookmarkCreationOptions: URL.BookmarkCreationOptions = [.minimalBookmark]
#endif

        do {
            let bookmark = try standardized.bookmarkData(
                options: bookmarkCreationOptions,
                includingResourceValuesForKeys: nil,
                relativeTo: nil
            )
            defaults.set(bookmark, forKey: defaultsSharedFolderBookmarkKey)

            try FileManager.default.createDirectory(at: standardized, withIntermediateDirectories: true)

            if started {
                standardized.stopAccessingSecurityScopedResource()
            }

            _ = beginPersistentSharedRootAccess(for: standardized)

            sharedRoot = standardized
            sharedDirectoryDisplayName = standardized.lastPathComponent
            fsProvider = IOSFilesystemProvider(rootURL: standardized)

            if !isInitialized {
                infoMessage = "Shared folder updated. It will apply after core starts."
                return
            }

            let provider = fsProvider
            runInBackground { [weak self] in
                do {
                    try registerFilesystemProvider(callback: provider)
                    Task { @MainActor [weak self] in
                        self?.infoMessage = "Shared folder updated"
                    }
                } catch {
                    Task { @MainActor [weak self] in
                        self?.lastErrorMessage = "Failed to register shared folder: \(error.localizedDescription)"
                    }
                }
            }
        } catch {
            if started {
                standardized.stopAccessingSecurityScopedResource()
            }
            lastErrorMessage = "Invalid shared folder: \(error.localizedDescription)"
        }
    }

    func resetSharedDirectoryToDefault() {
        defaults.removeObject(forKey: defaultsSharedFolderBookmarkKey)
        endPersistentSharedRootAccess()

        do {
            try FileManager.default.createDirectory(at: defaultSharedRoot, withIntermediateDirectories: true)
            sharedRoot = defaultSharedRoot
            sharedDirectoryDisplayName = defaultSharedRoot.lastPathComponent
            fsProvider = IOSFilesystemProvider(rootURL: defaultSharedRoot)
        } catch {
            lastErrorMessage = "Failed to reset shared folder: \(error.localizedDescription)"
            return
        }

        if !isInitialized {
            infoMessage = "Shared folder reset to default."
            return
        }

        let provider = fsProvider
        runInBackground { [weak self] in
            do {
                try registerFilesystemProvider(callback: provider)
                Task { @MainActor [weak self] in
                    self?.infoMessage = "Shared folder reset to default"
                }
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Failed to register shared folder: \(error.localizedDescription)"
                }
            }
        }
    }

    func sendMediaStateToActiveDevice() {
        guard let device = activeDevice else { return }
        let state = MediaState(
            title: mediaTitleDraft.isEmpty ? nil : mediaTitleDraft,
            artist: mediaArtistDraft.isEmpty ? nil : mediaArtistDraft,
            album: mediaAlbumDraft.isEmpty ? nil : mediaAlbumDraft,
            playing: mediaPlayingDraft
        )

        runInBackground { [weak self] in
            do {
                try sendMediaState(targetIp: device.ip, targetPort: device.port, state: state)
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Send media state failed: \(error.localizedDescription)"
                }
            }
        }
    }

    private func sendMediaStateToTrustedDevices(_ state: MediaState) {
        guard isInitialized, isMediaControlEnabled else { return }
        guard lastBroadcastLocalMediaState != state else { return }

        let targets = devices.filter { device in
            trustedDeviceIds.contains(device.id) && device.port != 0 && device.ip != "0.0.0.0"
        }
        guard !targets.isEmpty else { return }

        lastBroadcastLocalMediaState = state

        runInBackground { [weak self] in
            for device in targets {
                do {
                    try sendMediaState(targetIp: device.ip, targetPort: device.port, state: state)
                } catch {
                    Task { @MainActor [weak self] in
                        self?.lastErrorMessage = "Send media state failed: \(error.localizedDescription)"
                    }
                }
            }
        }
    }

    private func sendLastLocalMediaState(to device: DiscoveredDevice) {
        guard let state = latestLocalMediaState else { return }
        guard isMediaControlEnabled, device.port != 0, device.ip != "0.0.0.0" else { return }

        runInBackground { [weak self] in
            do {
                try sendMediaState(targetIp: device.ip, targetPort: device.port, state: state)
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Send media state failed: \(error.localizedDescription)"
                }
            }
        }
    }

    private func refreshLocalMediaStateAndBroadcast() {
#if canImport(MediaPlayer)
        guard isMediaControlEnabled else { return }

        let status = MPMediaLibrary.authorizationStatus()
        mediaLibraryPermissionGranted = status == .authorized
        guard status == .authorized else { return }

        let player = MPMusicPlayerController.systemMusicPlayer
        let item = player.nowPlayingItem
        let state = MediaState(
            title: normalizedMediaString(item?.title),
            artist: normalizedMediaString(item?.artist),
            album: normalizedMediaString(item?.albumTitle),
            playing: player.playbackState == .playing
        )

        latestLocalMediaState = state
        currentMediaState = state
        mediaTitleDraft = state.title ?? ""
        mediaArtistDraft = state.artist ?? ""
        mediaAlbumDraft = state.album ?? ""
        mediaPlayingDraft = state.playing
        sendMediaStateToTrustedDevices(state)
#endif
    }

    private func normalizedMediaString(_ value: String?) -> String? {
        let trimmed = value?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return trimmed.isEmpty ? nil : trimmed
    }

    func requestMessagesForConversation(_ conversation: FfiConversation, limit: UInt32 = 5_000) {
        guard let device = activeDevice else { return }
        requestMessages(for: conversation, from: device, limit: limit)
    }

    func requestMessages(for conversation: FfiConversation, from device: DiscoveredDevice, limit: UInt32 = 5_000) {
        messages = []
        selectedConversationThreadId = conversation.id
        runInBackground { [weak self] in
            do {
                try Connected.requestMessages(
                    targetIp: device.ip,
                    targetPort: device.port,
                    threadId: conversation.id,
                    limit: limit
                )
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Messages request failed: \(error.localizedDescription)"
                }
            }
        }
    }

    func sendSmsFromDraft(to number: String, body: String) {
        guard let device = activeDevice else { return }
        guard !number.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            lastErrorMessage = "Recipient is required"
            return
        }
        guard !body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            lastErrorMessage = "Message body is required"
            return
        }

        runInBackground { [weak self] in
            do {
                try sendSms(
                    targetIp: device.ip,
                    targetPort: device.port,
                    to: number,
                    body: body
                )
                Task { @MainActor [weak self] in
                    self?.infoMessage = "SMS request sent"
                }
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Send SMS failed: \(error.localizedDescription)"
                }
            }
        }
    }

    func sendCallActionToActiveDevice(_ action: CallAction) {
        guard let device = activeDevice else { return }

        runInBackground { [weak self] in
            do {
                try sendCallAction(targetIp: device.ip, targetPort: device.port, action: action)
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Call action failed: \(error.localizedDescription)"
                }
            }
        }
    }

    func initiateCallOnActiveDevice(number: String) {
        guard let device = activeDevice else { return }
        let trimmed = number.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            lastErrorMessage = "Phone number is required"
            return
        }

        runInBackground { [weak self] in
            do {
                try initiateCall(targetIp: device.ip, targetPort: device.port, number: trimmed)
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Initiate call failed: \(error.localizedDescription)"
                }
            }
        }
    }

    func sendActiveCallUpdateToActiveDevice(_ call: FfiActiveCall?) {
        guard let device = activeDevice else { return }

        runInBackground { [weak self] in
            do {
                try sendActiveCallUpdate(targetIp: device.ip, targetPort: device.port, call: call)
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Active call update failed: \(error.localizedDescription)"
                }
            }
        }
    }

    func browseRemoteFiles(_ device: DiscoveredDevice, path: String = "/") {
        browse(device: device, path: path)
    }

    func closeRemoteBrowser() {
        browsingDevice = nil
        remoteFiles = []
        currentRemotePath = "/"
        browserDownloadProgress = nil
        requestedThumbnails.removeAll()
    }

    func getThumbnail(path: String) {
        guard thumbnailDataByPath[path] == nil else { return }
        guard !requestedThumbnails.contains(path) else { return }
        guard let device = browsingDevice ?? activeDevice else { return }

        requestedThumbnails.insert(path)

        runInBackground { [weak self] in
            do {
                let data = try requestGetThumbnail(targetIp: device.ip, targetPort: device.port, path: path)
                Task { @MainActor [weak self] in
                    guard let self else { return }
                    self.requestedThumbnails.remove(path)
                    if !data.isEmpty {
                        self.thumbnailDataByPath[path] = data
                    }
                }
            } catch {
                Task { @MainActor [weak self] in
                    self?.requestedThumbnails.remove(path)
                }
            }
        }
    }

    func clearThumbnailCache() {
        thumbnailDataByPath.removeAll()
        requestedThumbnails.removeAll()
    }

    func refreshDevicesNow() {
        refreshDiscoveryNow()
    }

    func dismissInfoMessage() {
        infoMessage = nil
    }

    func setPendingShare(urls: [URL]) {
        pendingShareURLs = urls
    }

    func clearPendingShare() {
        pendingShareURLs = []
    }

    func sendPendingShare(to device: DiscoveredDevice) {
        let queued = pendingShareURLs
        clearPendingShare()
        for url in queued {
            sendFileToDevice(at: url, to: device)
        }
    }

    func sendClipboardFromPasteboard(to device: DiscoveredDevice) {
#if canImport(UIKit)
        guard let text = currentPasteboardText() else {
            infoMessage = "Clipboard is empty."
            return
        }
        clipboardContent = text
        sendClipboardText(text, to: device)
#endif
    }

    func sendClipboardTextToAllTrusted(_ text: String) {
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            infoMessage = "Clipboard is empty."
            return
        }

        let targets = devices.filter { trustedDeviceIds.contains($0.id) }
        guard !targets.isEmpty else {
            infoMessage = "No trusted devices to share clipboard with."
            return
        }

        clipboardContent = text
        shouldHighlightPasteAndShare = false
        for device in targets {
            sendClipboardText(text, to: device)
        }
    }

    func requestPasteAndShareFocus() {
        selectedRootTab = .devices
        shouldHighlightPasteAndShare = true
        infoMessage = "Tap Paste & Share to send the current clipboard."
    }

    func presentError(_ message: String) {
        lastErrorMessage = message
    }

    func clearError() {
        lastErrorMessage = nil
    }

    private func runInBackground(_ work: @Sendable @escaping () -> Void) {
        DispatchQueue.global(qos: .userInitiated).async(execute: work)
    }

    private func mergeDevices(_ incoming: [DiscoveredDevice]) {
        for device in incoming {
            handleDeviceFound(device)
        }
    }

    fileprivate func handleDeviceFound(_ device: DiscoveredDevice) {
        if let idx = devices.firstIndex(where: { $0.id == device.id }) {
            devices[idx] = device
        } else {
            devices.append(device)
        }

        devices.sort { lhs, rhs in
            lhs.name.localizedCaseInsensitiveCompare(rhs.name) == .orderedAscending
        }

        if trustedDeviceIds.contains(device.id) {
            sendLastLocalMediaState(to: device)
        }

        refreshTrustState(deviceId: device.id)
    }

    fileprivate func handleDeviceLost(_ deviceId: String) {
        devices.removeAll { $0.id == deviceId }
        trustedDeviceIds.remove(deviceId)
        pendingPairing.remove(deviceId)
        if selectedDeviceId == deviceId {
            selectedDeviceId = nil
        }
        if browsingDevice?.id == deviceId {
            browsingDevice = nil
            remoteFiles = []
            currentRemotePath = "/"
        }
    }

    fileprivate func handleDiscoveryError(_ message: String) {
        lastErrorMessage = "Discovery error: \(message)"
    }

    fileprivate func handlePairingRequest(deviceName: String, fingerprint: String, deviceId: String) {
        let wasPending = pendingPairing.contains(deviceId)
        let alreadyTrusted = trustedDeviceIds.contains(deviceId) || isDeviceTrusted(deviceId: deviceId)

        if wasPending || alreadyTrusted {
            pendingPairing.insert(deviceId)
            acceptPairingRequest(
                deviceName: deviceName,
                fingerprint: fingerprint,
                deviceId: deviceId,
                wasAlreadyTrusted: alreadyTrusted
            )
            return
        }

        if !pairingModeEnabled {
            rejectPairingRequestFromUnknownDevice(deviceName: deviceName, deviceId: deviceId)
            return
        }

        pendingPairing.insert(deviceId)
        pairingRequest = PairingPrompt(deviceName: deviceName, fingerprint: fingerprint, deviceId: deviceId)
    }

    fileprivate func handlePairingRejected(deviceName: String, deviceId: String) {
        pendingPairing.remove(deviceId)
        infoMessage = "Pairing rejected by \(deviceName)"
    }

    fileprivate func handlePairingModeChanged(enabled: Bool) {
        pairingModeEnabled = enabled
    }

    fileprivate func handleClipboardReceived(text: String, fromDevice: String) {
        clipboardContent = text
        lastRemoteClipboard = text
#if canImport(UIKit)
        if UIPasteboard.general.string != text {
            UIPasteboard.general.string = text
        }
#endif
        infoMessage = "Clipboard received from \(fromDevice)"
        showClipboardReceivedNotification(fromDevice: fromDevice)
    }

    fileprivate func handleClipboardSent(success: Bool, error: String?) {
        if success {
            infoMessage = "Clipboard sent"
        } else {
            lastErrorMessage = error ?? "Clipboard send failed"
        }
    }

    fileprivate func handleTransferRequest(transferId: String, filename: String, fileSize: UInt64, fromDevice: String) {
        transferRequest = TransferPrompt(
            transferId: transferId,
            filename: filename,
            fileSize: fileSize,
            fromDevice: fromDevice
        )
        transferStatus = "Incoming transfer request"
        let formatted = ByteCountFormatter.string(fromByteCount: Int64(fileSize), countStyle: .file)
        showTransferNotification(
            title: "Incoming File",
            body: "\(fromDevice) wants to send \(filename) (\(formatted)).",
            sound: true,
            requiresAcknowledgment: false
        )
    }

    fileprivate func handleTransferStarting(transferId: String, filename: String, totalSize: UInt64) {
        activeTransferId = transferId
        lastTransferNotificationProgressBucket = nil
        transferStatus = "Transferring \(filename)"
        showTransferNotification(
            title: "Transferring",
            body: filename,
            sound: false,
            requiresAcknowledgment: false
        )
    }

    fileprivate func handleTransferProgress(bytesTransferred: UInt64, totalSize: UInt64) {
        if totalSize > 0 {
            let pct = Int((Double(bytesTransferred) / Double(totalSize)) * 100)
            transferStatus = "Transfer \(pct)%"
            let bucket = min(10, pct / 10)
            if lastTransferNotificationProgressBucket != bucket {
                lastTransferNotificationProgressBucket = bucket
                showTransferNotification(
                    title: "Transferring",
                    body: "\(pct)% complete",
                    sound: false,
                    requiresAcknowledgment: false
                )
            }
        } else {
            transferStatus = "Transfer in progress"
            if lastTransferNotificationProgressBucket == nil {
                lastTransferNotificationProgressBucket = 0
                showTransferNotification(
                    title: "Transferring",
                    body: "Transfer in progress",
                    sound: false,
                    requiresAcknowledgment: false
                )
            }
        }
    }

    fileprivate func handleTransferCompleted(filename: String, totalSize: UInt64) {
        activeTransferId = nil
        lastTransferNotificationProgressBucket = nil
        transferStatus = "Completed \(filename)"
        let formatted = ByteCountFormatter.string(fromByteCount: Int64(totalSize), countStyle: .file)
        infoMessage = "Transfer complete (\(formatted))"
        showTransferNotification(
            title: "File Received",
            body: "\(filename) saved to \(downloadDirectoryDisplayName).",
            sound: true,
            requiresAcknowledgment: true
        )
    }

    fileprivate func handleTransferFailed(_ error: String) {
        activeTransferId = nil
        lastTransferNotificationProgressBucket = nil
        transferStatus = "Transfer failed"
        lastErrorMessage = error
        showTransferNotification(
            title: "Transfer Failed",
            body: error,
            sound: true,
            requiresAcknowledgment: true
        )
    }

    fileprivate func handleTransferCancelled() {
        activeTransferId = nil
        lastTransferNotificationProgressBucket = nil
        transferStatus = "Transfer cancelled"
        clearTransferNotification()
    }

    fileprivate func handleCompressionProgress(filename: String, currentFile: String, filesProcessed: UInt64, totalFiles: UInt64) {
        transferStatus = "Compressing \(currentFile) (\(filesProcessed)/\(max(totalFiles, 1)))"
        if !filename.isEmpty {
            infoMessage = "Preparing \(filename)"
        }
    }

    fileprivate func handleDeviceUnpaired(deviceId: String, deviceName: String, reason: String) {
        trustedDeviceIds.remove(deviceId)
        pendingPairing.remove(deviceId)
        infoMessage = "\(deviceName) \(reason)"
    }

    fileprivate func handleMediaCommand(fromDevice: String, command: MediaCommand) {
        guard isMediaControlEnabled else {
            infoMessage = "Ignored media command from \(fromDevice) because media control is disabled."
            return
        }

        if executeMediaCommand(command) {
            infoMessage = "Media command from \(fromDevice): \(String(describing: command))"
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak self] in
                self?.refreshLocalMediaStateAndBroadcast()
            }
        }
    }

    private func executeMediaCommand(_ command: MediaCommand) -> Bool {
#if canImport(MediaPlayer)
        let player = MPMusicPlayerController.systemMusicPlayer

        switch command {
        case .play:
            player.play()
        case .pause:
            player.pause()
        case .playPause:
            if player.playbackState == .playing {
                player.pause()
            } else {
                player.play()
            }
        case .next:
            player.skipToNextItem()
        case .previous:
            player.skipToPreviousItem()
        case .stop:
            player.stop()
        case .volumeUp, .volumeDown:
            infoMessage = "iOS does not allow apps to execute system volume commands."
            return false
        }

        return true
#else
        infoMessage = "Media commands are unavailable on this platform."
        return false
#endif
    }

    fileprivate func handleMediaState(fromDevice: String, state: MediaState) {
        currentMediaState = state
        infoMessage = "Media update from \(fromDevice)"
    }

    fileprivate func handleContactsReceived(_ contacts: [FfiContact]) {
        self.contacts = contacts
    }

    fileprivate func handleConversationsReceived(_ conversations: [FfiConversation]) {
        self.conversations = conversations
    }

    fileprivate func handleMessagesReceived(_ messages: [FfiSmsMessage]) {
        self.messages = messages
    }

    fileprivate func handleCallLogReceived(_ entries: [FfiCallLogEntry]) {
        callLog = entries
    }

    fileprivate func handleActiveCallUpdate(_ call: FfiActiveCall?) {
        activeCall = call
    }

    fileprivate func handleSmsSendResult(success: Bool, messageId: String?, error: String?) {
        if success {
            infoMessage = "SMS sent\(messageId.map { " (\($0))" } ?? "")"
        } else {
            lastErrorMessage = error ?? "SMS send failed"
        }
    }

    fileprivate func handleNewSms(_ message: FfiSmsMessage) {
        messages.insert(message, at: 0)
    }

    fileprivate func handleDownloadProgress(bytesDownloaded: UInt64, totalBytes: UInt64, currentFile: String) {
        browserDownloadProgress = BrowserDownloadProgressState(
            currentFile: currentFile,
            bytesDownloaded: bytesDownloaded,
            totalBytes: max(totalBytes, 1),
            isFolder: false
        )
    }

    fileprivate func handleDownloadCompleted(totalBytes: UInt64) {
        let formatted = ByteCountFormatter.string(fromByteCount: Int64(totalBytes), countStyle: .file)
        browserDownloadProgress = nil
        infoMessage = "Download complete (\(formatted))"
    }

    fileprivate func handleDownloadFailed(error: String) {
        browserDownloadProgress = nil
        lastErrorMessage = "Download failed: \(error)"
    }

    private func acceptPairingRequest(
        deviceName: String,
        fingerprint: String,
        deviceId: String,
        wasAlreadyTrusted: Bool
    ) {
        let device = devices.first(where: { $0.id == deviceId })

        runInBackground { [weak self] in
            do {
                if fingerprint != "Verified (You initiated)" {
                    try trustDevice(fingerprint: fingerprint, deviceId: deviceId, name: deviceName)
                }

                if let device, device.port != 0, device.ip != "0.0.0.0" {
                    try sendTrustConfirmation(targetIp: device.ip, targetPort: device.port)
                }

                Task { @MainActor [weak self] in
                    guard let self else { return }
                    self.trustedDeviceIds.insert(deviceId)
                    self.pendingPairing.remove(deviceId)
                    if self.pairingRequest?.deviceId == deviceId {
                        self.pairingRequest = nil
                    }
                    self.infoMessage = wasAlreadyTrusted
                        ? "Accepted trusted pairing from \(deviceName)"
                        : "Paired with \(deviceName)"
                }
            } catch {
                Task { @MainActor [weak self] in
                    self?.pendingPairing.remove(deviceId)
                    self?.lastErrorMessage = "Pairing accept failed: \(error.localizedDescription)"
                }
            }
        }
    }

    private func rejectPairingRequestFromUnknownDevice(deviceName: String, deviceId: String) {
        runInBackground { [weak self] in
            do {
                try rejectPairing(deviceId: deviceId)
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Reject pairing failed: \(error.localizedDescription)"
                }
            }

            Task { @MainActor [weak self] in
                self?.pendingPairing.remove(deviceId)
                if self?.pairingRequest?.deviceId == deviceId {
                    self?.pairingRequest = nil
                }
                self?.infoMessage = "Rejected pairing from \(deviceName) because pairing mode is off"
            }
        }
    }

    fileprivate func respondToContactsRequest(fromIp: String, fromPort: UInt16) {
        runInBackground { [weak self] in
            do {
                let contacts = try Self.loadLocalContacts()
                try sendContacts(targetIp: fromIp, targetPort: fromPort, contacts: contacts)
                Task { @MainActor [weak self] in
                    self?.contacts = contacts
                    self?.contactsPermissionGranted = true
                    self?.infoMessage = "Sent \(contacts.count) contacts"
                }
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Contacts response failed: \(error.localizedDescription)"
                }
            }
        }
    }

    fileprivate func respondToConversationsRequest(fromIp: String, fromPort: UInt16) {
        runInBackground { [weak self] in
            do {
                try sendConversations(targetIp: fromIp, targetPort: fromPort, conversations: [])
                Task { @MainActor [weak self] in
                    self?.infoMessage = "iOS does not expose SMS conversations to apps"
                }
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Conversations response failed: \(error.localizedDescription)"
                }
            }
        }
    }

    fileprivate func respondToMessagesRequest(fromIp: String, fromPort: UInt16, threadId: String) {
        runInBackground { [weak self] in
            do {
                try sendMessages(targetIp: fromIp, targetPort: fromPort, threadId: threadId, messages: [])
                Task { @MainActor [weak self] in
                    self?.infoMessage = "iOS does not expose SMS messages to apps"
                }
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Messages response failed: \(error.localizedDescription)"
                }
            }
        }
    }

    fileprivate func respondToSmsSendRequest(fromIp: String, fromPort: UInt16) {
        runInBackground { [weak self] in
            do {
                try sendSmsSendResult(
                    targetIp: fromIp,
                    targetPort: fromPort,
                    success: false,
                    messageId: nil,
                    error: "iOS telephony provider is not available in this build"
                )
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "SMS response failed: \(error.localizedDescription)"
                }
            }
        }
    }

    fileprivate func respondToCallLogRequest(fromIp: String, fromPort: UInt16) {
        runInBackground { [weak self] in
            do {
                try sendCallLog(targetIp: fromIp, targetPort: fromPort, entries: [])
                Task { @MainActor [weak self] in
                    self?.infoMessage = "iOS does not expose call history to apps"
                }
            } catch {
                Task { @MainActor [weak self] in
                    self?.lastErrorMessage = "Call log response failed: \(error.localizedDescription)"
                }
            }
        }
    }

    private func refreshTrustState(deviceId: String) {
        runInBackground { [weak self] in
            let trusted = isDeviceTrusted(deviceId: deviceId)
            Task { @MainActor [weak self] in
                guard let self else { return }
                if trusted {
                    self.trustedDeviceIds.insert(deviceId)
                    if let device = self.devices.first(where: { $0.id == deviceId }) {
                        self.sendLastLocalMediaState(to: device)
                    }
                } else {
                    self.trustedDeviceIds.remove(deviceId)
                }
            }
        }
    }

    nonisolated private static func loadLocalContacts() throws -> [FfiContact] {
#if canImport(Contacts)
        guard CNContactStore.authorizationStatus(for: .contacts) == .authorized else {
            throw NSError(
                domain: "ConnectedIOS",
                code: 1,
                userInfo: [NSLocalizedDescriptionKey: "Contacts permission not granted"]
            )
        }

        let store = CNContactStore()
        let keys: [CNKeyDescriptor] = [
            CNContactIdentifierKey as CNKeyDescriptor,
            CNContactFormatter.descriptorForRequiredKeys(for: .fullName),
            CNContactNicknameKey as CNKeyDescriptor,
            CNContactOrganizationNameKey as CNKeyDescriptor,
            CNContactPhoneNumbersKey as CNKeyDescriptor,
            CNContactEmailAddressesKey as CNKeyDescriptor
        ]
        let request = CNContactFetchRequest(keysToFetch: keys)
        request.sortOrder = .userDefault

        var contacts: [FfiContact] = []
        try store.enumerateContacts(with: request) { contact, _ in
            let formattedName = CNContactFormatter.string(from: contact, style: .fullName)?
                .trimmingCharacters(in: .whitespacesAndNewlines)
            let fallbackName = [contact.nickname, contact.organizationName, contact.identifier]
                .first { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
                ?? contact.identifier

            let phoneNumbers = contact.phoneNumbers.map { labeledNumber in
                FfiPhoneNumber(
                    number: labeledNumber.value.stringValue,
                    label: phoneNumberType(for: labeledNumber.label)
                )
            }

            let emails = contact.emailAddresses.map { String($0.value) }

            contacts.append(
                FfiContact(
                    id: contact.identifier,
                    name: formattedName?.isEmpty == false ? (formattedName ?? fallbackName) : fallbackName,
                    phoneNumbers: phoneNumbers,
                    emails: emails,
                    photo: nil,
                    starred: false
                )
            )
        }

        return contacts
#else
        return []
#endif
    }

#if canImport(Contacts)
    nonisolated private static func phoneNumberType(for label: String?) -> PhoneNumberType {
        guard let label else { return .other }

        if label == CNLabelPhoneNumberMobile || label == CNLabelPhoneNumberiPhone {
            return .mobile
        }
        if label == CNLabelHome {
            return .home
        }
        if label == CNLabelWork {
            return .work
        }
        if label == CNLabelPhoneNumberMain {
            return .main
        }
        return .other
    }
#endif

    private static func resolveFolderBookmark(
        fileManager: FileManager,
        defaults: UserDefaults,
        defaultsKey: String
    ) -> URL? {
        guard let bookmarkData = defaults.data(forKey: defaultsKey) else {
            return nil
        }

#if os(macOS)
        let bookmarkCreationOptions: URL.BookmarkCreationOptions = [.withSecurityScope]
        let bookmarkResolutionOptions: URL.BookmarkResolutionOptions = [.withSecurityScope]
#else
        let bookmarkCreationOptions: URL.BookmarkCreationOptions = [.minimalBookmark]
        let bookmarkResolutionOptions: URL.BookmarkResolutionOptions = []
#endif

        var stale = false
        do {
            let restored = try URL(
                resolvingBookmarkData: bookmarkData,
                options: bookmarkResolutionOptions,
                relativeTo: nil,
                bookmarkDataIsStale: &stale
            ).standardizedFileURL

            if stale {
                let refreshedBookmark = try restored.bookmarkData(
                    options: bookmarkCreationOptions,
                    includingResourceValuesForKeys: nil,
                    relativeTo: nil
                )
                defaults.set(refreshedBookmark, forKey: defaultsKey)
            }

            try fileManager.createDirectory(at: restored, withIntermediateDirectories: true)
            return restored
        } catch {
            defaults.removeObject(forKey: defaultsKey)
            return nil
        }
    }

    @discardableResult
    private func beginPersistentDownloadRootAccess(for url: URL) -> Bool {
        let standardized = url.standardizedFileURL
        if securityScopedDownloadRootURL?.path == standardized.path {
            return true
        }

        endPersistentDownloadRootAccess()
        guard standardized.startAccessingSecurityScopedResource() else {
            return false
        }

        securityScopedDownloadRootURL = standardized
        return true
    }

    private func endPersistentDownloadRootAccess() {
        guard let scopedURL = securityScopedDownloadRootURL else { return }
        scopedURL.stopAccessingSecurityScopedResource()
        securityScopedDownloadRootURL = nil
    }

    @discardableResult
    private func beginPersistentSharedRootAccess(for url: URL) -> Bool {
        let standardized = url.standardizedFileURL
        if securityScopedSharedRootURL?.path == standardized.path {
            return true
        }

        endPersistentSharedRootAccess()
        guard standardized.startAccessingSecurityScopedResource() else {
            return false
        }

        securityScopedSharedRootURL = standardized
        return true
    }

    private func endPersistentSharedRootAccess() {
        guard let scopedURL = securityScopedSharedRootURL else { return }
        scopedURL.stopAccessingSecurityScopedResource()
        securityScopedSharedRootURL = nil
    }

    private func beginBackgroundNetworkingTask() {
#if canImport(UIKit)
        guard backgroundTaskIdentifier == .invalid else { return }

        backgroundTaskIdentifier = UIApplication.shared.beginBackgroundTask(withName: "ConnectedNetworking") { [weak self] in
            Task { @MainActor [weak self] in
                self?.endBackgroundNetworkingTask()
                self?.infoMessage = "iOS paused Connected background networking."
            }
        }
#endif
    }

    private func endBackgroundNetworkingTask() {
#if canImport(UIKit)
        guard backgroundTaskIdentifier != .invalid else { return }
        let taskIdentifier = backgroundTaskIdentifier
        backgroundTaskIdentifier = .invalid
        UIApplication.shared.endBackgroundTask(taskIdentifier)
#endif
    }

    private func startLocalMediaObserverIfNeeded() {
#if canImport(MediaPlayer)
        guard mediaPlaybackObservers.isEmpty else { return }

        let status = MPMediaLibrary.authorizationStatus()
        mediaLibraryPermissionGranted = status == .authorized
        guard status == .authorized else { return }

        let player = MPMusicPlayerController.systemMusicPlayer
        player.beginGeneratingPlaybackNotifications()

        let center = NotificationCenter.default
        let itemObserver = center.addObserver(
            forName: .MPMusicPlayerControllerNowPlayingItemDidChange,
            object: player,
            queue: .main
        ) { [weak self] _ in
            Task { @MainActor [weak self] in
                self?.refreshLocalMediaStateAndBroadcast()
            }
        }
        let playbackObserver = center.addObserver(
            forName: .MPMusicPlayerControllerPlaybackStateDidChange,
            object: player,
            queue: .main
        ) { [weak self] _ in
            Task { @MainActor [weak self] in
                self?.refreshLocalMediaStateAndBroadcast()
            }
        }

        mediaPlaybackObservers = [itemObserver, playbackObserver]
        refreshLocalMediaStateAndBroadcast()
#endif
    }

    private func stopLocalMediaObserver() {
#if canImport(MediaPlayer)
        for observer in mediaPlaybackObservers {
            NotificationCenter.default.removeObserver(observer)
        }
        mediaPlaybackObservers = []
        MPMusicPlayerController.systemMusicPlayer.endGeneratingPlaybackNotifications()
#endif
    }

#if canImport(UIKit)
    private func startClipboardMonitorIfNeeded() {
        guard clipboardObserver == nil else { return }

        clipboardObserver = NotificationCenter.default.addObserver(
            forName: UIPasteboard.changedNotification,
            object: UIPasteboard.general,
            queue: .main
        ) { [weak self] _ in
            Task { @MainActor [weak self] in
                self?.handleClipboardChanged()
            }
        }
    }

    private func handleClipboardChanged() {
        guard isClipboardSyncEnabled else { return }
        guard let current = currentPasteboardText() else { return }

        if current == lastRemoteClipboard {
            lastRemoteClipboard = ""
            return
        }

        sendClipboardTextToAllTrusted(current)
    }

    private func currentPasteboardText() -> String? {
        guard let value = UIPasteboard.general.string else { return nil }
        return value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? nil : value
    }
#endif

    fileprivate func handleClipboardNotificationAction() {
        initializeIfNeeded()
        requestPasteAndShareFocus()
    }

    private func refreshNotificationAuthorizationStatus() {
#if canImport(UserNotifications)
        UNUserNotificationCenter.current().getNotificationSettings { [weak self] settings in
            let granted: Bool
            switch settings.authorizationStatus {
            case .authorized, .provisional, .ephemeral:
                granted = true
            default:
                granted = false
            }

            Task { @MainActor [weak self] in
                self?.notificationsPermissionGranted = granted
            }
        }
#else
        notificationsPermissionGranted = false
#endif
    }

    private func registerClipboardNotificationCategory() {
#if canImport(UserNotifications)
        let shareAction = UNNotificationAction(
            identifier: clipboardNotificationActionIdentifier,
            title: "Paste & Share",
            options: [.foreground]
        )
        let category = UNNotificationCategory(
            identifier: clipboardNotificationCategoryIdentifier,
            actions: [shareAction],
            intentIdentifiers: [],
            options: []
        )
        UNUserNotificationCenter.current().setNotificationCategories([category])
#endif
    }

    private func scheduleClipboardShareNotificationIfNeeded() {
#if canImport(UserNotifications)
        guard isClipboardSyncEnabled, hasTrustedDevices else {
            cancelClipboardShareNotification()
            return
        }

        UNUserNotificationCenter.current().getNotificationSettings { [weak self] settings in
            let authorized: Bool
            switch settings.authorizationStatus {
            case .authorized, .provisional, .ephemeral:
                authorized = true
            default:
                authorized = false
            }
            guard authorized else { return }

            Task { @MainActor [weak self] in
                guard let self, self.isClipboardSyncEnabled, self.hasTrustedDevices else { return }

                let content = UNMutableNotificationContent()
                content.title = "Connected"
                content.body = "Tap to open Paste & Share for trusted devices."
                content.categoryIdentifier = self.clipboardNotificationCategoryIdentifier
                content.sound = nil

                let trigger = UNTimeIntervalNotificationTrigger(timeInterval: 1, repeats: false)
                let request = UNNotificationRequest(
                    identifier: self.clipboardNotificationIdentifier,
                    content: content,
                    trigger: trigger
                )

                let center = UNUserNotificationCenter.current()
                center.removePendingNotificationRequests(withIdentifiers: [self.clipboardNotificationIdentifier])
                do {
                    try await center.add(request)
                } catch {
                    self.lastErrorMessage = "Clipboard notification failed: \(error.localizedDescription)"
                }
            }
        }
#endif
    }

    private func cancelClipboardShareNotification() {
#if canImport(UserNotifications)
        UNUserNotificationCenter.current().removePendingNotificationRequests(withIdentifiers: [clipboardNotificationIdentifier])
        UNUserNotificationCenter.current().removeDeliveredNotifications(withIdentifiers: [clipboardNotificationIdentifier])
#endif
    }

    private func showClipboardReceivedNotification(fromDevice: String) {
#if canImport(UserNotifications)
        UNUserNotificationCenter.current().getNotificationSettings { [weak self] settings in
            let authorized: Bool
            switch settings.authorizationStatus {
            case .authorized, .provisional, .ephemeral:
                authorized = true
            default:
                authorized = false
            }
            guard authorized else { return }

            Task { @MainActor [weak self] in
                guard let self else { return }
                let content = UNMutableNotificationContent()
                content.title = "Clipboard Received"
                content.body = "Received clipboard text from \(fromDevice)."
                content.threadIdentifier = "connected.clipboard"
                content.sound = nil

                let request = UNNotificationRequest(
                    identifier: self.clipboardReceivedNotificationIdentifier,
                    content: content,
                    trigger: nil
                )

                let center = UNUserNotificationCenter.current()
                center.removePendingNotificationRequests(withIdentifiers: [self.clipboardReceivedNotificationIdentifier])
                center.removeDeliveredNotifications(withIdentifiers: [self.clipboardReceivedNotificationIdentifier])
                do {
                    try await center.add(request)
                } catch {
                    self.lastErrorMessage = "Clipboard notification failed: \(error.localizedDescription)"
                }
            }
        }
#endif
    }

    private func showTransferNotification(title: String, body: String, sound: Bool, requiresAcknowledgment: Bool) {
#if canImport(UserNotifications)
        UNUserNotificationCenter.current().getNotificationSettings { [weak self] settings in
            let authorized: Bool
            switch settings.authorizationStatus {
            case .authorized, .provisional, .ephemeral:
                authorized = true
            default:
                authorized = false
            }
            guard authorized else { return }

            Task { @MainActor [weak self] in
                guard let self else { return }
                if requiresAcknowledgment {
                    self.transferNotificationNeedsAcknowledgment = true
                }

                let content = UNMutableNotificationContent()
                content.title = title
                content.body = body
                content.threadIdentifier = "connected.transfer"
                content.sound = sound ? .default : nil

                let request = UNNotificationRequest(
                    identifier: self.transferNotificationIdentifier,
                    content: content,
                    trigger: nil
                )

                let center = UNUserNotificationCenter.current()
                center.removePendingNotificationRequests(withIdentifiers: [self.transferNotificationIdentifier])
                center.removeDeliveredNotifications(withIdentifiers: [self.transferNotificationIdentifier])
                do {
                    try await center.add(request)
                } catch {
                    self.lastErrorMessage = "Transfer notification failed: \(error.localizedDescription)"
                }
            }
        }
#endif
    }

    private func acknowledgeTransferNotificationIfNeeded() {
#if canImport(UserNotifications)
        guard transferNotificationNeedsAcknowledgment else { return }
        transferNotificationNeedsAcknowledgment = false
        clearTransferNotification()
#endif
    }

    private func clearTransferNotification() {
#if canImport(UserNotifications)
        UNUserNotificationCenter.current().removePendingNotificationRequests(withIdentifiers: [transferNotificationIdentifier])
        UNUserNotificationCenter.current().removeDeliveredNotifications(withIdentifiers: [transferNotificationIdentifier])
#endif
    }
}

#if canImport(UserNotifications)
private final class ClipboardNotificationBridge: NSObject, UNUserNotificationCenterDelegate, @unchecked Sendable {
    weak var app: ConnectedAppModel?

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        let isClipboardNotification = response.notification.request.identifier == "connected.clipboard.share"
        let isShareAction = response.actionIdentifier == "connected.clipboard.share.action"
            || response.actionIdentifier == UNNotificationDefaultActionIdentifier

        if isClipboardNotification && isShareAction {
            Task { @MainActor [weak app] in
                app?.handleClipboardNotificationAction()
                completionHandler()
            }
        } else {
            completionHandler()
        }
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        completionHandler([.banner, .list])
    }
}
#endif

private final class DiscoveryBridge: DiscoveryCallback, @unchecked Sendable {
    weak var app: ConnectedAppModel?

    func onDeviceFound(device: DiscoveredDevice) {
        Task { @MainActor [weak app] in
            app?.handleDeviceFound(device)
        }
    }

    func onDeviceLost(deviceId: String) {
        Task { @MainActor [weak app] in
            app?.handleDeviceLost(deviceId)
        }
    }

    func onError(errorMsg: String) {
        Task { @MainActor [weak app] in
            app?.handleDiscoveryError(errorMsg)
        }
    }
}

private final class ClipboardBridge: ClipboardCallback, @unchecked Sendable {
    weak var app: ConnectedAppModel?

    func onClipboardReceived(text: String, fromDevice: String) {
        Task { @MainActor [weak app] in
            app?.handleClipboardReceived(text: text, fromDevice: fromDevice)
        }
    }

    func onClipboardSent(success: Bool, errorMsg: String?) {
        Task { @MainActor [weak app] in
            app?.handleClipboardSent(success: success, error: errorMsg)
        }
    }
}

private final class TransferBridge: FileTransferCallback, @unchecked Sendable {
    weak var app: ConnectedAppModel?

    func onTransferRequest(transferId: String, filename: String, fileSize: UInt64, fromDevice: String) {
        Task { @MainActor [weak app] in
            app?.handleTransferRequest(
                transferId: transferId,
                filename: filename,
                fileSize: fileSize,
                fromDevice: fromDevice
            )
        }
    }

    func onTransferStarting(transferId: String, filename: String, totalSize: UInt64) {
        Task { @MainActor [weak app] in
            app?.handleTransferStarting(transferId: transferId, filename: filename, totalSize: totalSize)
        }
    }

    func onTransferProgress(bytesTransferred: UInt64, totalSize: UInt64) {
        Task { @MainActor [weak app] in
            app?.handleTransferProgress(bytesTransferred: bytesTransferred, totalSize: totalSize)
        }
    }

    func onTransferCompleted(filename: String, totalSize: UInt64) {
        Task { @MainActor [weak app] in
            app?.handleTransferCompleted(filename: filename, totalSize: totalSize)
        }
    }

    func onTransferFailed(errorMsg: String) {
        Task { @MainActor [weak app] in
            app?.handleTransferFailed(errorMsg)
        }
    }

    func onTransferCancelled() {
        Task { @MainActor [weak app] in
            app?.handleTransferCancelled()
        }
    }

    func onCompressionProgress(filename: String, currentFile: String, filesProcessed: UInt64, totalFiles: UInt64, bytesProcessed: UInt64, totalBytes: UInt64, speedBytesPerSec: UInt64) {
        Task { @MainActor [weak app] in
            app?.handleCompressionProgress(
                filename: filename,
                currentFile: currentFile,
                filesProcessed: filesProcessed,
                totalFiles: totalFiles
            )
        }
    }
}

private final class PairingBridge: PairingCallback, @unchecked Sendable {
    weak var app: ConnectedAppModel?

    func onPairingRequest(deviceName: String, fingerprint: String, deviceId: String) {
        Task { @MainActor [weak app] in
            app?.handlePairingRequest(deviceName: deviceName, fingerprint: fingerprint, deviceId: deviceId)
        }
    }

    func onPairingRejected(deviceName: String, deviceId: String) {
        Task { @MainActor [weak app] in
            app?.handlePairingRejected(deviceName: deviceName, deviceId: deviceId)
        }
    }

    func onPairingModeChanged(enabled: Bool) {
        Task { @MainActor [weak app] in
            app?.handlePairingModeChanged(enabled: enabled)
        }
    }
}

private final class UnpairBridge: UnpairCallback, @unchecked Sendable {
    weak var app: ConnectedAppModel?

    func onDeviceUnpaired(deviceId: String, deviceName: String, reason: String) {
        Task { @MainActor [weak app] in
            app?.handleDeviceUnpaired(deviceId: deviceId, deviceName: deviceName, reason: reason)
        }
    }
}

private final class MediaBridge: MediaControlCallback, @unchecked Sendable {
    weak var app: ConnectedAppModel?

    func onMediaCommand(fromDevice: String, command: MediaCommand) {
        Task { @MainActor [weak app] in
            app?.handleMediaCommand(fromDevice: fromDevice, command: command)
        }
    }

    func onMediaStateUpdate(fromDevice: String, state: MediaState) {
        Task { @MainActor [weak app] in
            app?.handleMediaState(fromDevice: fromDevice, state: state)
        }
    }
}

private final class TelephonyBridge: TelephonyCallback, @unchecked Sendable {
    weak var app: ConnectedAppModel?

    func onContactsSyncRequest(fromDevice: String, fromIp: String, fromPort: UInt16) {
        Task { @MainActor [weak app] in
            app?.respondToContactsRequest(fromIp: fromIp, fromPort: fromPort)
        }
    }

    func onContactsReceived(fromDevice: String, contacts: [FfiContact]) {
        Task { @MainActor [weak app] in
            app?.handleContactsReceived(contacts)
        }
    }

    func onConversationsSyncRequest(fromDevice: String, fromIp: String, fromPort: UInt16) {
        Task { @MainActor [weak app] in
            app?.respondToConversationsRequest(fromIp: fromIp, fromPort: fromPort)
        }
    }

    func onConversationsReceived(fromDevice: String, conversations: [FfiConversation]) {
        Task { @MainActor [weak app] in
            app?.handleConversationsReceived(conversations)
        }
    }

    func onMessagesRequest(fromDevice: String, fromIp: String, fromPort: UInt16, threadId: String, limit: UInt32) {
        Task { @MainActor [weak app] in
            app?.respondToMessagesRequest(fromIp: fromIp, fromPort: fromPort, threadId: threadId)
        }
    }

    func onMessagesReceived(fromDevice: String, threadId: String, messages: [FfiSmsMessage]) {
        Task { @MainActor [weak app] in
            app?.handleMessagesReceived(messages)
        }
    }

    func onSendSmsRequest(fromDevice: String, fromIp: String, fromPort: UInt16, to: String, body: String) {
        Task { @MainActor [weak app] in
            app?.respondToSmsSendRequest(fromIp: fromIp, fromPort: fromPort)
        }
    }

    func onSmsSendResult(success: Bool, messageId: String?, error: String?) {
        Task { @MainActor [weak app] in
            app?.handleSmsSendResult(success: success, messageId: messageId, error: error)
        }
    }

    func onNewSms(fromDevice: String, message: FfiSmsMessage) {
        Task { @MainActor [weak app] in
            app?.handleNewSms(message)
        }
    }

    func onCallLogRequest(fromDevice: String, fromIp: String, fromPort: UInt16, limit: UInt32) {
        Task { @MainActor [weak app] in
            app?.respondToCallLogRequest(fromIp: fromIp, fromPort: fromPort)
        }
    }

    func onCallLogReceived(fromDevice: String, entries: [FfiCallLogEntry]) {
        Task { @MainActor [weak app] in
            app?.handleCallLogReceived(entries)
        }
    }

    func onInitiateCallRequest(fromDevice: String, fromIp: String, fromPort: UInt16, number: String) {
        Task { @MainActor [weak app] in
            app?.infoMessage = "Remote call request for \(number) is not supported on iOS yet"
        }
    }

    func onCallActionRequest(fromDevice: String, fromIp: String, fromPort: UInt16, action: CallAction) {
        Task { @MainActor [weak app] in
            app?.infoMessage = "Remote call action \(String(describing: action)) is not supported on iOS yet"
        }
    }

    func onActiveCallUpdate(fromDevice: String, call: FfiActiveCall?) {
        Task { @MainActor [weak app] in
            app?.handleActiveCallUpdate(call)
        }
    }
}

private final class BrowserDownloadBridge: BrowserDownloadCallback, @unchecked Sendable {
    weak var app: ConnectedAppModel?

    func onDownloadProgress(bytesDownloaded: UInt64, totalBytes: UInt64, currentFile: String) {
        Task { @MainActor [weak app] in
            app?.handleDownloadProgress(bytesDownloaded: bytesDownloaded, totalBytes: totalBytes, currentFile: currentFile)
        }
    }

    func onDownloadCompleted(totalBytes: UInt64) {
        Task { @MainActor [weak app] in
            app?.handleDownloadCompleted(totalBytes: totalBytes)
        }
    }

    func onDownloadFailed(errorMsg: String) {
        Task { @MainActor [weak app] in
            app?.handleDownloadFailed(error: errorMsg)
        }
    }
}
