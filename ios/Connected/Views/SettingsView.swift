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
    @Environment(\.colorScheme) private var colorScheme
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
            .glassBackground()
            .toolbar(.hidden, for: .navigationBar)
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

    // MARK: - Identity Card
    private var identityCard: some View {
        GlassSettingsCard(
            title: "Device Name",
            subtitle: "This name will be visible to other devices.",
            icon: "person.circle"
        ) {
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 10) {
                    TextField("Device name", text: $editableName)
                        .textFieldStyle(.roundedBorder)
                        .foregroundStyle(GlassTheme.onSurface(for: colorScheme))

                    Button("Save") {
                        model.setDeviceName(editableName)
                    }
                    .glassButtonProminent()
                }
            }
        }
    }

    // MARK: - Pairing Card
    private var pairingCard: some View {
        GlassSettingsCard(
            title: "Pairing",
            subtitle: "Control whether new devices can request trust.",
            icon: "checkmark.shield"
        ) {
            VStack(alignment: .leading, spacing: 12) {
                HStack {
                    Label("Pairing mode", systemImage: "checkmark.shield")
                        .foregroundStyle(GlassTheme.onSurface(for: colorScheme))
                    Spacer()
                    Toggle("", isOn: $pairingMode)
                        .labelsHidden()
                        .toggleStyle(MonochromeToggleStyle())
                        .onChangeCompat(of: pairingMode) { enabled in
                            model.updatePairingMode(enabled: enabled)
                        }
                }

                Button("Refresh Discovery") {
                    model.refreshDiscoveryNow()
                }
                .glassButtonProminent()
            }
        }
    }

    // MARK: - Download Location Card
    private var receivedFilesCard: some View {
        GlassSettingsCard(
            title: "Download Location",
            subtitle: "Choose where received files are saved.",
            icon: "arrow.down.circle"
        ) {
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 10) {
                    Text(model.downloadDirectoryDisplayName)
                        .lineLimit(1)
                        .foregroundStyle(GlassTheme.onSurface(for: colorScheme))
                    Spacer()
                    Button("Browse") {
                        folderPickerTarget = .downloads
                        showingFolderPicker = true
                    }
                    .glassButtonProminent()
                }

                if model.downloadRoot.path != model.defaultDownloadRoot.path {
                    Button("Reset to Default", role: .destructive) {
                        model.resetDownloadDirectoryToDefault()
                    }
                    .glassButton()
                }

                if let progress = model.browserDownloadProgress {
                    ProgressView(value: progress.fractionCompleted)
                    Text(progress.currentFile)
                        .font(.caption)
                        .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme))
                }
            }
        }
    }

    // MARK: - Shared Folder Card
    private var sharedFolderCard: some View {
        GlassSettingsCard(
            title: "Shared Folder",
            subtitle: "Choose which local folder peers can browse.",
            icon: "folder"
        ) {
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 10) {
                    Text(model.sharedDirectoryDisplayName)
                        .lineLimit(1)
                        .foregroundStyle(GlassTheme.onSurface(for: colorScheme))
                    Spacer()
                    Button("Browse") {
                        folderPickerTarget = .shared
                        showingFolderPicker = true
                    }
                    .glassButtonProminent()
                }

                if model.sharedRoot.path != model.defaultSharedRoot.path {
                    Button("Reset to Default", role: .destructive) {
                        model.resetSharedDirectoryToDefault()
                    }
                    .glassButton()
                }
            }
        }
    }

    // MARK: - Preferences Card
    private var preferencesCard: some View {
        GlassSettingsCard(
            title: "Preferences",
            subtitle: "Tune local behavior and appearance.",
            icon: "slider.horizontal.3"
        ) {
            VStack(alignment: .leading, spacing: 14) {
                // Theme Picker
                VStack(alignment: .leading, spacing: 6) {
                    Text("Appearance")
                        .font(.subheadline)
                        .foregroundStyle(GlassTheme.onSurface(for: colorScheme))

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
                }

                GlassToggleRow(
                    title: "Clipboard Sync",
                    subtitle: "Send clipboard changes while Connected is active.",
                    systemImage: "doc.on.clipboard",
                    isOn: Binding(
                        get: { model.isClipboardSyncEnabled },
                        set: { model.setClipboardSyncEnabled($0) }
                    )
                )

                GlassToggleRow(
                    title: "Media Control",
                    subtitle: "Allow trusted devices to control supported playback.",
                    systemImage: "play.circle",
                    isOn: Binding(
                        get: { model.isMediaControlEnabled },
                        set: { model.setMediaControlEnabled($0) }
                    )
                )

                GlassToggleRow(
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

    // MARK: - Background Clipboard Card
    private var backgroundClipboardCard: some View {
        GlassSettingsCard(
            title: "Background Clipboard",
            subtitle: "iOS only lets apps read clipboard while active.",
            icon: "bell"
        ) {
            VStack(alignment: .leading, spacing: 12) {
                Text("Connected can post a helper notification while backgrounded. Open Connected from the helper notification to show Paste & Share, or click on device options and Share Clipboard.")
                    .font(.footnote)
                    .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme))

                if model.notificationsPermissionGranted {
                    Label("Notification helper enabled", systemImage: "checkmark.seal")
                        .font(.footnote)
                        .foregroundStyle(GlassTheme.success)
                } else {
                    Button("Enable Notification Helper") {
                        model.requestClipboardNotificationPermission()
                    }
                    .glassButton()
                }
            }
        }
    }

    // MARK: - Permissions Card
    private var permissionsCard: some View {
        GlassSettingsCard(
            title: "Permissions",
            subtitle: "Grant access for optional platform features.",
            icon: "lock.shield"
        ) {
            VStack(alignment: .leading, spacing: 12) {
                if model.contactsPermissionGranted {
                    Label("Contacts access granted", systemImage: "checkmark.seal")
                        .foregroundStyle(GlassTheme.success)
                } else {
                    Button("Request Contacts Access") {
                        model.requestContactsPermission()
                    }
                    .glassButtonProminent()
                }

                if model.mediaLibraryPermissionGranted {
                    Label("Apple Music access granted", systemImage: "checkmark.seal")
                        .foregroundStyle(GlassTheme.success)
                } else {
                    Button("Request Apple Music Access") {
                        model.requestMediaLibraryPermission()
                    }
                    .glassButtonProminent()
                }

                Text("iOS can report Apple Music/system music player metadata. It cannot read now-playing metadata from arbitrary third-party apps.")
                    .font(.caption)
                    .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme))
            }
        }
    }
}

// MARK: - Glass Settings Card
private struct GlassSettingsCard<Content: View>: View {
    let title: String
    let subtitle: String?
    let icon: String
    let content: Content

    @Environment(\.colorScheme) private var colorScheme

    init(title: String, subtitle: String? = nil, icon: String = "gearshape", @ViewBuilder content: () -> Content) {
        self.title = title
        self.subtitle = subtitle
        self.icon = icon
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            // Header with icon (matching Android card style)
            HStack(spacing: 12) {
                Image(systemName: icon)
                    .font(.body)
                    .foregroundStyle(GlassTheme.primary(for: colorScheme))
                    .frame(width: 24, height: 24)

                VStack(alignment: .leading, spacing: 2) {
                    Text(title)
                        .font(.headline)
                        .foregroundStyle(GlassTheme.onSurface(for: colorScheme))
                    if let subtitle, !subtitle.isEmpty {
                        Text(subtitle)
                            .font(.footnote)
                            .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme))
                    }
                }
            }

            content
        }
        .glassCard()
    }
}

// MARK: - Glass Toggle Row
private struct GlassToggleRow: View {
    let title: String
    let subtitle: String
    let systemImage: String
    @Binding var isOn: Bool

    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: systemImage)
                .font(.body)
                .foregroundStyle(GlassTheme.primary(for: colorScheme))
                .frame(width: 24, height: 24)

            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.body)
                    .foregroundStyle(GlassTheme.onSurface(for: colorScheme))
                Text(subtitle)
                    .font(.caption)
                    .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme))
            }

            Spacer()

            Toggle(title, isOn: $isOn)
                .labelsHidden()
                .toggleStyle(MonochromeToggleStyle())
        }
    }
}
