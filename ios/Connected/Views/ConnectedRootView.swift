import SwiftUI

struct ConnectedRootView: View {
    @EnvironmentObject private var model: ConnectedAppModel
    @Environment(\.colorScheme) private var colorScheme

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
        .overlay {
            if let request = model.pairingRequest {
                Color.black.opacity(0.4)
                    .ignoresSafeArea()
                    .onTapGesture {
                        model.rejectCurrentPairingRequest()
                    }

                VStack(spacing: 0) {
                    Text("Pairing Request")
                        .font(.title3.bold())
                        .padding(.top, 24)

                    VStack(alignment: .leading, spacing: 8) {
                        Text("A device wants to connect to your iPhone.")
                            .font(.body)

                        Divider()

                        HStack {
                            Text("Device:")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            Spacer()
                            Text(request.deviceName)
                                .font(.caption)
                                .fontWeight(.medium)
                        }

                        HStack {
                            Text("ID:")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            Spacer()
                            Text(String(request.fingerprint.prefix(16)) + "...")
                                .font(.caption)
                                .monospaced()
                        }
                    }
                    .padding(.horizontal, 20)
                    .padding(.top, 12)

                    Divider()
                        .padding(.top, 16)

                    Button {
                        model.trustCurrentPairingRequest()
                    } label: {
                        Text("Trust")
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 12)
                    }

                    Divider()

                    Button(role: .destructive) {
                        model.rejectCurrentPairingRequest()
                    } label: {
                        Text("Reject")
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 12)
                    }
                }
                .background(.regularMaterial)
                .clipShape(RoundedRectangle(cornerRadius: 14))
                .padding(.horizontal, 40)
            }
        }
        .alert("Incoming Transfer", isPresented: transferPresented, presenting: model.transferRequest) { request in
            Button("Accept") {
                model.acceptCurrentTransferRequest()
            }
            .glassButtonProminent()

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
        .glassTabBar()
        .tint(colorScheme == .dark ? .white : .black)
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
