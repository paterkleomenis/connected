import Foundation
import UIKit
import SwiftUI

/// Manages AirDrop protocol support on iOS, making Connected discoverable
/// by Apple devices and enabling file reception from stock AirDrop.
///
/// The AirDrop protocol runs over HTTPS with three endpoints:
/// - /Discover: Device discovery handshake
/// - /Ask: File transfer request (sender asks, receiver accepts/rejects)
/// - /Upload: File transfer (CPIO archive upload)
///
/// On iOS, the actual AirDrop HTTPS server runs in the Rust core via FFI.
/// This manager bridges the Swift UI to the Rust AirDrop service.
@MainActor
final class AirDropManager: ObservableObject {
    @Published var isServerRunning = false
    @Published var isDiscoverable = true
    @Published var serverPort: UInt16 = 0
    @Published var receivedFiles: [AirDropReceivedFile] = []
    @Published var lastError: String?

    private weak var appModel: ConnectedAppModel?

    init(appModel: ConnectedAppModel? = nil) {
        self.appModel = appModel
    }

    /// Start the AirDrop server, making this device discoverable by Apple devices.
    func startServer() {
        guard !isServerRunning else { return }

        let downloadDir = FileManager.default.urls(for: .downloadsDirectory, in: .userDomainMask).first!
            .appendingPathComponent("Connected", isDirectory: true).path

        Task.detached { [weak self] in
            do {
                let port = try startAirdropServer(downloadDir: downloadDir)
                await MainActor.run { [weak self] in
                    self?.isServerRunning = true
                    self?.serverPort = port
                    self?.lastError = nil
                    print("[AirDrop] Server started on port \(port)")
                }
            } catch {
                await MainActor.run { [weak self] in
                    self?.lastError = "Failed to start AirDrop server: \(error.localizedDescription)"
                    print("[AirDrop] Server start failed: \(error)")
                }
            }
        }
    }

    /// Stop the AirDrop server and unregister from mDNS.
    func stopServer() {
        stopAirdropServer()
        isServerRunning = false
        serverPort = 0
        print("[AirDrop] Server stopped")
    }

    /// Handle an incoming AirDrop transfer request.
    func handleTransferRequest(request: AirDropTransferRequest) {
        print("[AirDrop] Transfer request from '\(request.senderName)': \(request.fileName) (\(request.fileSize) bytes)")

        let file = AirDropReceivedFile(
            id: request.transferId,
            senderName: request.senderName,
            fileName: request.fileName,
            fileSize: request.fileSize,
            status: .pending,
            receivedAt: Date()
        )
        receivedFiles.append(file)
    }

    /// Handle a completed AirDrop transfer.
    func handleTransferCompleted(result: AirDropTransferCompleted) {
        print("[AirDrop] Transfer completed: \(result.filePath)")

        if let index = receivedFiles.firstIndex(where: { $0.id == result.transferId }) {
            receivedFiles[index].status = .completed
            receivedFiles[index].localPath = result.filePath
        }
    }

    /// Handle a failed AirDrop transfer.
    func handleTransferFailed(result: AirDropTransferFailed) {
        print("[AirDrop] Transfer failed: \(result.error)")

        if let index = receivedFiles.firstIndex(where: { $0.id == result.transferId }) {
            receivedFiles[index].status = .failed
            receivedFiles[index].error = result.error
        }
    }

    /// Handle an AirDrop-capable device being discovered.
    func handleDeviceFound(device: AirDropDeviceFound) {
        print("[AirDrop] Device found: \(device.deviceName) at \(device.ip):\(device.port)")
    }

    /// Handle an AirDrop device being lost.
    func handleDeviceLost(deviceId: String) {
        print("[AirDrop] Device lost: \(deviceId)")
    }

    /// Handle AirDrop errors.
    func handleError(errorMsg: String) {
        lastError = errorMsg
        print("[AirDrop] Error: \(errorMsg)")
    }

    /// Open a received file in the appropriate app.
    func openFile(_ file: AirDropReceivedFile) {
        guard let path = file.localPath else { return }
        let url = URL(fileURLWithPath: path)
        UIApplication.shared.open(url)
    }

    /// Share files via the system share sheet (which includes AirDrop).
    func shareViaSystem(files: [URL]) {
        guard let scene = UIApplication.shared.connectedScenes.first as? UIWindowScene,
              let rootVC = scene.windows.first?.rootViewController else { return }

        let activityVC = UIActivityViewController(activityItems: files, applicationActivities: nil)
        rootVC.present(activityVC, animated: true)
    }
}

// MARK: - Data Models

struct AirDropReceivedFile: Identifiable {
    let id: String
    let senderName: String
    let fileName: String
    let fileSize: UInt64
    var status: TransferStatus
    var localPath: String?
    var error: String?
    let receivedAt: Date

    enum TransferStatus {
        case pending
        case completed
        case failed
    }

    var fileSizeFormatted: String {
        ByteCountFormatter.string(fromByteCount: Int64(fileSize), countStyle: .file)
    }
}

// MARK: - AirDrop Bridge (implements the FFI callback interface)

/// Bridge between the Rust AirDrop service and the Swift UI.
/// Implements the AirDropCallback protocol from the FFI layer.
final class AirDropBridge: AirDropCallback, @unchecked Sendable {
    weak var app: ConnectedAppModel?

    func onTransferRequest(request: AirDropTransferRequest) {
        Task { @MainActor [weak app] in
            app?.airDropManager.handleTransferRequest(request: request)
        }
    }

    func onTransferCompleted(result: AirDropTransferCompleted) {
        Task { @MainActor [weak app] in
            app?.airDropManager.handleTransferCompleted(result: result)
        }
    }

    func onTransferFailed(result: AirDropTransferFailed) {
        Task { @MainActor [weak app] in
            app?.airDropManager.handleTransferFailed(result: result)
        }
    }

    func onDeviceFound(device: AirDropDeviceFound) {
        Task { @MainActor [weak app] in
            app?.airDropManager.handleDeviceFound(device: device)
        }
    }

    func onDeviceLost(deviceId: String) {
        Task { @MainActor [weak app] in
            app?.airDropManager.handleDeviceLost(deviceId: deviceId)
        }
    }

    func onError(errorMsg: String) {
        Task { @MainActor [weak app] in
            app?.airDropManager.handleError(errorMsg: errorMsg)
        }
    }
}
