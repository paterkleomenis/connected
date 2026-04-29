import SwiftUI
import UniformTypeIdentifiers
#if canImport(UIKit)
import UIKit
#endif

struct SettingsView: View {
    private enum FolderPickerTarget {
        case downloads
        case shared
    }

    @EnvironmentObject private var model: ConnectedAppModel
    @State private var editableName = ""
    @State private var pairingMode = false
    @State private var showingFolderPicker = false
    @State private var folderPickerTarget: FolderPickerTarget?

    var body: some View {
        NavigationStack {
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 16) {
                    identityCard
                    pairingCard
                    receivedFilesCard
                    sharedFolderCard
                    preferencesCard
                    backgroundClipboardCard
                    permissionsCard
                }
                .padding(16)
            }
            .background(screenBackground.ignoresSafeArea())
            .navigationTitle("Settings")
            .onAppear {
                editableName = model.deviceName
                pairingMode = model.pairingModeEnabled
            }
            .onChangeCompat(of: model.deviceName) { newValue in
                editableName = newValue
            }
            .onChangeCompat(of: model.pairingModeEnabled) { newValue in
                pairingMode = newValue
            }
            .fileImporter(
                isPresented: $showingFolderPicker,
                allowedContentTypes: [UTType.folder],
                allowsMultipleSelection: false
            ) { result in
                defer { folderPickerTarget = nil }

                switch result {
                case .success(let urls):
                    guard let url = urls.first else { return }
                    switch folderPickerTarget {
                    case .downloads:
                        model.setDownloadDirectoryFromURL(url)
                    case .shared:
                        model.setSharedDirectoryFromURL(url)
                    case nil:
                        return
                    }
                case .failure(let error):
                    model.presentError("Folder pick failed: \(error.localizedDescription)")
                }
            }
        }
    }

    private var identityCard: some View {
        SettingsCard(title: "Device Name", subtitle: "This name will be visible to other devices.") {
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 10) {
                    TextField("Device name", text: $editableName)
                        .textFieldStyle(.roundedBorder)

                    Button("Save") {
                        model.setDeviceName(editableName)
                    }
                    .buttonStyle(.borderedProminent)
                }

                if let local = model.localDevice {
                    InfoRow(label: "Local ID", value: local.id)
                    InfoRow(label: "Address", value: "\(local.ip):\(local.port)")
                }

                if !model.localFingerprint.isEmpty {
                    InfoRow(label: "Fingerprint", value: model.localFingerprint)
                }
            }
        }
    }

    private var pairingCard: some View {
        SettingsCard(title: "Pairing", subtitle: "Control whether new devices can request trust.") {
            VStack(alignment: .leading, spacing: 12) {
                HStack {
                    Label("Pairing mode", systemImage: "checkmark.shield")
                    Spacer()
                    Toggle("", isOn: $pairingMode)
                        .labelsHidden()
                        .onChangeCompat(of: pairingMode) { enabled in
                            model.updatePairingMode(enabled: enabled)
                        }
                }

                Button("Refresh Discovery") {
                    model.refreshDiscoveryNow()
                }
                .buttonStyle(.borderedProminent)
                .frame(maxWidth: .infinity)
            }
        }
    }

    private var receivedFilesCard: some View {
        SettingsCard(title: "Download Location", subtitle: "Choose where received files are saved.") {
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 10) {
                    Text(model.downloadDirectoryDisplayName)
                        .lineLimit(1)
                    Spacer()
                    Button("Browse") {
                        folderPickerTarget = .downloads
                        showingFolderPicker = true
                    }
                    .buttonStyle(.borderedProminent)
                }

                if model.downloadRoot.path != model.defaultDownloadRoot.path {
                    Button("Reset to Default", role: .destructive) {
                        model.resetDownloadDirectoryToDefault()
                    }
                    .buttonStyle(.bordered)
                    .frame(maxWidth: .infinity)
                }

                Text(model.downloadRoot.path)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)

                if let progress = model.browserDownloadProgress {
                    ProgressView(value: progress.fractionCompleted)
                    Text(progress.currentFile)
                        .font(.caption)
                }
            }
        }
    }

    private var sharedFolderCard: some View {
        SettingsCard(title: "Shared Folder", subtitle: "Choose which local folder peers can browse.") {
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 10) {
                    Text(model.sharedDirectoryDisplayName)
                        .lineLimit(1)
                    Spacer()
                    Button("Browse") {
                        folderPickerTarget = .shared
                        showingFolderPicker = true
                    }
                    .buttonStyle(.borderedProminent)
                }

                if model.sharedRoot.path != model.defaultSharedRoot.path {
                    Button("Reset to Default", role: .destructive) {
                        model.resetSharedDirectoryToDefault()
                    }
                    .buttonStyle(.bordered)
                    .frame(maxWidth: .infinity)
                }

                Text(model.sharedRoot.path)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
            }
        }
    }

    private var preferencesCard: some View {
        SettingsCard(title: "Preferences", subtitle: "Tune local behavior and appearance.") {
            VStack(alignment: .leading, spacing: 14) {
                Picker(
                    "Theme",
                    selection: Binding(
                        get: { model.themeMode },
                        set: { model.setThemeMode($0) }
                    )
                ) {
                    Text("System").tag(ConnectedAppModel.ThemeMode.system)
                    Text("Light").tag(ConnectedAppModel.ThemeMode.light)
                    Text("Dark").tag(ConnectedAppModel.ThemeMode.dark)
                }
                .pickerStyle(.segmented)

                ToggleRow(
                    title: "Clipboard Sync",
                    subtitle: "Send clipboard changes while Connected is active.",
                    systemImage: "doc.on.clipboard",
                    isOn: Binding(
                        get: { model.isClipboardSyncEnabled },
                        set: { model.setClipboardSyncEnabled($0) }
                    )
                )

                ToggleRow(
                    title: "Media Control",
                    subtitle: "Allow trusted devices to control supported playback.",
                    systemImage: "play.circle",
                    isOn: Binding(
                        get: { model.isMediaControlEnabled },
                        set: { model.setMediaControlEnabled($0) }
                    )
                )

                ToggleRow(
                    title: "Telephony",
                    subtitle: "Expose supported contact and call surfaces.",
                    systemImage: "phone",
                    isOn: Binding(
                        get: { model.isTelephonyEnabled },
                        set: { model.setTelephonyEnabled($0) }
                    )
                )
            }
        }
    }

    private var backgroundClipboardCard: some View {
        SettingsCard(title: "Background Clipboard", subtitle: "iOS only lets apps read clipboard while active. Connected can post a helper notification while backgrounded.") {
            VStack(alignment: .leading, spacing: 12) {
                Button("Open Paste & Share") {
                    model.requestPasteAndShareFocus()
                }
                .buttonStyle(.borderedProminent)
                .frame(maxWidth: .infinity)
                .disabled(!model.hasTrustedDevices)

                if model.notificationsPermissionGranted {
                    Label("Notification helper enabled", systemImage: "checkmark.seal")
                        .font(.footnote)
                        .foregroundStyle(.green)
                } else {
                    Button("Enable Notification Helper") {
                        model.requestClipboardNotificationPermission()
                    }
                    .buttonStyle(.bordered)
                    .frame(maxWidth: .infinity)
                }
            }
        }
    }

    private var permissionsCard: some View {
        SettingsCard(title: "Permissions", subtitle: "Grant access for optional platform features.") {
            VStack(alignment: .leading, spacing: 12) {
                if model.contactsPermissionGranted {
                    Label("Contacts access granted", systemImage: "checkmark.seal")
                        .foregroundStyle(.green)
                } else {
                    Button("Request Contacts Access") {
                        model.requestContactsPermission()
                    }
                    .buttonStyle(.borderedProminent)
                    .frame(maxWidth: .infinity)
                }

                if model.mediaLibraryPermissionGranted {
                    Label("Apple Music access granted", systemImage: "checkmark.seal")
                        .foregroundStyle(.green)
                } else {
                    Button("Request Apple Music Access") {
                        model.requestMediaLibraryPermission()
                    }
                    .buttonStyle(.borderedProminent)
                    .frame(maxWidth: .infinity)
                }

                Text("iOS can report Apple Music/system music player metadata. It cannot read now-playing metadata from arbitrary third-party apps.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }

    private var screenBackground: Color {
#if canImport(UIKit)
        Color(uiColor: .systemGroupedBackground)
#else
        Color.clear
#endif
    }
}

private struct SettingsCard<Content: View>: View {
    let title: String
    let subtitle: String?
    let content: Content

    init(title: String, subtitle: String? = nil, @ViewBuilder content: () -> Content) {
        self.title = title
        self.subtitle = subtitle
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                Text(title)
                    .font(.headline)
                if let subtitle, !subtitle.isEmpty {
                    Text(subtitle)
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
            }
            content
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
    }

    private var cardBackground: Color {
#if canImport(UIKit)
        Color(uiColor: .secondarySystemGroupedBackground)
#else
        Color.clear
#endif
    }
}

private struct InfoRow: View {
    let label: String
    let value: String

    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            Text(label)
                .font(.caption)
                .foregroundStyle(.secondary)
            Spacer(minLength: 12)
            Text(value)
                .font(.caption)
                .multilineTextAlignment(.trailing)
                .lineLimit(2)
        }
    }
}

private struct ToggleRow: View {
    let title: String
    let subtitle: String
    let systemImage: String
    @Binding var isOn: Bool

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: systemImage)
                .foregroundStyle(.tint)
                .frame(width: 24)
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.body)
                Text(subtitle)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            Toggle(title, isOn: $isOn)
                .labelsHidden()
        }
    }
}
