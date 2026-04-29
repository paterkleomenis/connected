import SwiftUI

struct ConnectedRootView: View {
    @EnvironmentObject private var model: ConnectedAppModel

    var body: some View {
        Group {
            if model.browsingDevice != nil {
                RemoteFilesView()
            } else {
                rootTabs
            }
        }
        .alert("Error", isPresented: errorPresented) {
            Button("OK") {
                model.clearError()
            }
        } message: {
            Text(model.lastErrorMessage ?? "Unknown error")
        }
        .alert("Pairing Request", isPresented: pairingPresented, presenting: model.pairingRequest) { request in
            Button("Trust") {
                model.trustCurrentPairingRequest()
            }
            Button("Reject", role: .destructive) {
                model.rejectCurrentPairingRequest()
            }
            Button("Later", role: .cancel) {
                model.pairingRequest = nil
            }
        } message: { request in
            Text("\(request.deviceName) wants to pair.\nFingerprint: \(request.fingerprint)")
        }
        .alert("Incoming Transfer", isPresented: transferPresented, presenting: model.transferRequest) { request in
            Button("Accept") {
                model.acceptCurrentTransferRequest()
            }
            Button("Reject", role: .destructive) {
                model.rejectCurrentTransferRequest()
            }
        } message: { request in
            let size = ByteCountFormatter.string(fromByteCount: Int64(request.fileSize), countStyle: .file)
            Text("\(request.fromDevice) sent \(request.filename) (\(size)).")
        }
    }

    private var rootTabs: some View {
        TabView(selection: $model.selectedRootTab) {
            DeviceListView()
                .tabItem {
                    Label("Devices", systemImage: "dot.radiowaves.left.and.right")
                }
                .tag(ConnectedAppModel.RootTab.devices)

            SettingsView()
                .tabItem {
                    Label("Settings", systemImage: "gearshape")
                }
                .tag(ConnectedAppModel.RootTab.settings)
        }
    }

    private var errorPresented: Binding<Bool> {
        Binding(
            get: { model.lastErrorMessage != nil },
            set: { shown in
                if !shown {
                    model.clearError()
                }
            }
        )
    }

    private var pairingPresented: Binding<Bool> {
        Binding(
            get: { model.pairingRequest != nil },
            set: { shown in
                if !shown {
                    model.pairingRequest = nil
                }
            }
        )
    }

    private var transferPresented: Binding<Bool> {
        Binding(
            get: { model.transferRequest != nil },
            set: { shown in
                if !shown {
                    model.transferRequest = nil
                }
            }
        )
    }
}
