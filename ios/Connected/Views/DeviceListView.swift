import SwiftUI
import UniformTypeIdentifiers
#if canImport(UIKit)
import UIKit
#endif

struct DeviceListView: View {
    private enum PhoneDataRoute: Identifiable {
        case contacts(DiscoveredDevice)
        case conversations(DiscoveredDevice)
        case callLog(DiscoveredDevice)

        var id: String {
            switch self {
            case .contacts(let device):
                return "contacts-\(device.id)"
            case .conversations(let device):
                return "conversations-\(device.id)"
            case .callLog(let device):
                return "call-log-\(device.id)"
            }
        }
    }

    @EnvironmentObject private var model: ConnectedAppModel
    @Environment(\.colorScheme) private var colorScheme
    @State private var showingFileImporter = false
    @State private var selectedFileTarget: DiscoveredDevice?
    @State private var phoneDataRoute: PhoneDataRoute?

    var body: some View {
        NavigationStack {
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 16) {
                    if !model.pendingShareURLs.isEmpty {
                        GlassStatusCard(
                            systemImage: "tray.full",
                            text: "\(model.pendingShareURLs.count) shared item(s) queued"
                        )
                    }

                    if model.isClipboardSectionVisible {
                        pasteAndShareControl
                    }

                    if model.devices.isEmpty {
                        GlassSectionCard(title: "Devices", subtitle: "Waiting for nearby peers.") {
                            VStack(spacing: 12) {
                                Image(systemName: "dot.radiowaves.left.and.right")
                                    .font(.system(size: 32))
                                    .foregroundStyle(GlassTheme.primary(for: colorScheme).opacity(0.5))
                                Text("No devices yet")
                                    .font(.headline)
                                    .foregroundStyle(GlassTheme.onSurface(for: colorScheme))
                                Text("Pull to refresh or start discovery")
                                    .font(.caption)
                                    .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme))
                            }
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 20)
                        }
                    } else {
                        LazyVStack(spacing: 10) {
                            ForEach(model.devices, id: \.id) { device in
                                DeviceRow(
                                    device: device,
                                    isTrusted: model.isTrusted(device),
                                    isPending: model.isPending(device),
                                    hasPendingShare: !model.pendingShareURLs.isEmpty,
                                    isMediaControlEnabled: model.isMediaControlEnabled,
                                    onPair: { model.requestPairing(with: device) },
                                    onCancelPair: { model.cancelPairing(with: device) },
                                    onUnpair: { model.unpairDevice(device) },
                                    onSendFile: {
                                        selectedFileTarget = device
                                        showingFileImporter = true
                                    },
                                    onSendPendingShare: { model.sendPendingShare(to: device) },
                                    onShareClipboard: { model.sendClipboardFromPasteboard(to: device) },
                                    onBrowse: { model.browseRemoteFiles(device) },
                                    onRequestContacts: {
                                        phoneDataRoute = .contacts(device)
                                        model.requestContacts(from: device)
                                    },
                                    onRequestConversations: {
                                        phoneDataRoute = .conversations(device)
                                        model.requestConversations(from: device)
                                    },
                                    onRequestCallLog: {
                                        phoneDataRoute = .callLog(device)
                                        model.requestCallLog(from: device)
                                    },
                                    onMediaCommand: { command in model.sendMediaCommand(command, to: device) }
                                )
                            }
                        }
                    }

                    if model.transferStatus != "Idle" {
                        transferCard
                    }
                }
                .padding(16)
            }
            .refreshable {
                model.refreshDiscoveryNow()
            }
            .glassBackground()
            .toolbar(.hidden, for: .navigationBar)
            .fileImporter(
                isPresented: $showingFileImporter,
                allowedContentTypes: [UTType.data],
                allowsMultipleSelection: false
            ) { result in
                guard let device = selectedFileTarget else { return }
                defer { selectedFileTarget = nil }

                switch result {
                case .success(let urls):
                    guard let url = urls.first else { return }
                    model.sendFileToDevice(at: url, to: device)
                case .failure(let error):
                    model.presentError("File pick failed: \(error.localizedDescription)")
                }
            }
            .sheet(item: $phoneDataRoute) { route in
                switch route {
                case .contacts(let device):
                    PhoneDataSheet(kind: .contacts, device: device)
                        .environmentObject(model)
                case .conversations(let device):
                    PhoneDataSheet(kind: .conversations, device: device)
                        .environmentObject(model)
                case .callLog(let device):
                    PhoneDataSheet(kind: .callLog, device: device)
                        .environmentObject(model)
                }
            }
        }
    }

    private var pasteAndShareControl: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                Image(systemName: "doc.on.clipboard.fill")
                    .font(.title3)
                    .foregroundStyle(GlassTheme.primary(for: colorScheme))
                    .frame(width: 28, height: 28)
                    .background(GlassTheme.primary(for: colorScheme).opacity(0.1))
                    .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))

                VStack(alignment: .leading, spacing: 2) {
                    Text("Paste & Share")
                        .font(.headline)
                        .foregroundStyle(GlassTheme.onSurface(for: colorScheme))
                    Text("Paste through iOS and send to every trusted device.")
                        .font(.caption)
                        .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme))
                }
            }

            PasteButton(payloadType: String.self) { strings in
                let text = strings
                    .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
                    .filter { !$0.isEmpty }
                    .joined(separator: "\n")
                model.sendClipboardTextToAllTrusted(text, hideClipboardSectionOnSuccess: true)
            }
            .controlSize(.large)
            .disabled(!model.hasTrustedDevices)

            if !model.hasTrustedDevices {
                Text("Pair and trust a device to enable Paste & Share.")
                    .font(.caption)
                    .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme))
            }
        }
        .glassCard()
        .overlay(
            RoundedRectangle(cornerRadius: 16, style: .continuous)
                .stroke(
                    model.shouldHighlightPasteAndShare
                        ? GlassTheme.accent(for: colorScheme)
                        : Color.clear,
                    lineWidth: model.shouldHighlightPasteAndShare ? 2 : 0
                )
        )
    }

    private var transferCard: some View {
        GlassSectionCard(title: "Transfer", subtitle: model.transferStatus) {
            HStack(spacing: 10) {
                if model.activeTransferId != nil {
                    Button("Cancel", role: .destructive) {
                        model.cancelActiveTransfer()
                    }
                    .glassButton()
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

// MARK: - Device Row (Matching Android DeviceItem)
private struct DeviceRow: View {
    let device: DiscoveredDevice
    let isTrusted: Bool
    let isPending: Bool
    let hasPendingShare: Bool
    let isMediaControlEnabled: Bool
    let onPair: () -> Void
    let onCancelPair: () -> Void
    let onUnpair: () -> Void
    let onSendFile: () -> Void
    let onSendPendingShare: () -> Void
    let onShareClipboard: () -> Void
    let onBrowse: () -> Void
    let onRequestContacts: () -> Void
    let onRequestConversations: () -> Void
    let onRequestCallLog: () -> Void
    let onMediaCommand: (MediaCommand) -> Void

    @Environment(\.colorScheme) private var colorScheme
    @State private var showDetails = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            // Top row: Device info and action buttons (matching Android DeviceItem)
            HStack(spacing: 12) {
                // Device icon
                Image(systemName: iconName(for: device.deviceType, name: device.name))
                    .font(.title3)
                    .foregroundStyle(GlassTheme.primary(for: colorScheme))
                    .frame(width: 24, height: 24)

                // Device info
                VStack(alignment: .leading, spacing: 3) {
                    Text(device.name)
                        .font(.body)
                        .foregroundStyle(GlassTheme.onSurface(for: colorScheme))
                        .lineLimit(1)

                    if showDetails {
                        Text("\(device.ip):\(device.port)")
                            .font(.caption)
                            .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme))
                    }

                    if isTrusted {
                        Label("Trusted", systemImage: "checkmark.shield.fill")
                            .font(.caption2)
                            .foregroundStyle(GlassTheme.success)
                    }
                }
                .onTapGesture {
                    withAnimation(.easeInOut(duration: 0.2)) {
                        showDetails.toggle()
                    }
                }

                Spacer(minLength: 8)

                // Action buttons (matching Android)
                if isTrusted {
                    HStack(spacing: 4) {
                        // Send file button
                        Button {
                            onSendFile()
                        } label: {
                            Image(systemName: "arrow.up.circle.fill")
                                .font(.title)
                                .frame(width: 56, height: 56)
                        }
                        .buttonStyle(.plain)
                        .foregroundStyle(GlassTheme.primary(for: colorScheme))

                        // Menu button
                        Menu {
                            if hasPendingShare {
                                Button("Send Queued Share", action: onSendPendingShare)
                            }
                            Button("Share Clipboard", action: onShareClipboard)
                            Button("Browse Files", action: onBrowse)
                            Button("Request Contacts", action: onRequestContacts)
                            if device.deviceType != .ios {
                                Button("Request Conversations", action: onRequestConversations)
                                Button("Request Call Log", action: onRequestCallLog)
                            }
                            Button("Unpair", role: .destructive, action: onUnpair)
                        } label: {
                            Image(systemName: "ellipsis.circle.fill")
                                .font(.title)
                                .frame(width: 56, height: 56)
                        }
                        .buttonStyle(.plain)
                        .foregroundStyle(GlassTheme.primary(for: colorScheme))
                    }
                } else {
                    HStack(spacing: 12) {
                        if isPending {
                            Button {
                                onCancelPair()
                            } label: {
                                Text("Cancel")
                                    .font(.subheadline)
                                    .frame(maxWidth: .infinity)
                                    .frame(height: 40)
                            }
                            .buttonStyle(.bordered)
                            .tint(.red)
                        } else {
                            Button {
                                onSendFile()
                            } label: {
                                Text("Send File")
                                    .font(.subheadline)
                                    .frame(maxWidth: .infinity)
                                    .frame(height: 40)
                            }
                            .buttonStyle(.bordered)
                        }

                        if isPending {
                            Button {
                            } label: {
                                Text("Waiting...")
                                    .font(.subheadline)
                                    .frame(maxWidth: .infinity)
                                    .frame(height: 40)
                            }
                            .buttonStyle(.bordered)
                            .disabled(true)
                        } else {
                            Button {
                                onPair()
                            } label: {
                                Text("Pair")
                                    .font(.subheadline)
                                    .frame(maxWidth: .infinity)
                                    .frame(height: 40)
                            }
                            .buttonStyle(.borderedProminent)
                            .tint(.black)
                        }
                    }
                }
            }
            .padding(.vertical, 12)

            // Media Controls row (below device info) for trusted devices
            if isTrusted && isMediaControlEnabled {
                Divider()
                    .overlay(GlassTheme.outlineVariant(for: colorScheme).opacity(0.5))

                HStack {
                    mediaButton(systemImage: "speaker.minus.fill", label: "Volume Down", command: .volumeDown)
                    Spacer()
                    mediaButton(systemImage: "backward.end.fill", label: "Previous", command: .previous)
                    Spacer()
                    mediaButton(systemImage: "playpause.fill", label: "Play/Pause", command: .playPause)
                    Spacer()
                    mediaButton(systemImage: "forward.end.fill", label: "Next", command: .next)
                    Spacer()
                    mediaButton(systemImage: "speaker.plus.fill", label: "Volume Up", command: .volumeUp)
                }
                .padding(.top, 8)
                .padding(.bottom, 4)
            }
        }
        .glassCard(padding: 12)
    }

    private func mediaButton(systemImage: String, label: String, command: MediaCommand) -> some View {
        Button {
            onMediaCommand(command)
        } label: {
            Image(systemName: systemImage)
                .font(.title3)
                .frame(width: 48, height: 48)
                .background(GlassTheme.primary(for: colorScheme).opacity(0.12))
                .clipShape(Circle())
        }
        .buttonStyle(.plain)
        .foregroundStyle(GlassTheme.primary(for: colorScheme))
        .accessibilityLabel(label)
    }

    private func iconName(for type: DeviceType, name: String? = nil) -> String {
        deviceIconName(for: type, name: name)
    }
}

// MARK: - Glass Section Card
private struct GlassSectionCard<Content: View>: View {
    let title: String
    let subtitle: String?
    let content: Content

    @Environment(\.colorScheme) private var colorScheme

    init(title: String, subtitle: String? = nil, @ViewBuilder content: () -> Content) {
        self.title = title
        self.subtitle = subtitle
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                Text(title)
                    .font(.headline)
                    .foregroundStyle(GlassTheme.onSurface(for: colorScheme))
                if let subtitle, !subtitle.isEmpty {
                    Text(subtitle)
                        .font(.footnote)
                        .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme))
                }
            }
            content
        }
        .glassCard()
    }
}

// MARK: - Glass Status Card
private struct GlassStatusCard: View {
    let systemImage: String
    let text: String

    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: systemImage)
                .font(.body)
                .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme))
                .frame(width: 20, height: 20)

            Text(text)
                .font(.footnote)
                .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme))
                .lineLimit(2)

            Spacer()
        }
        .glassCard(padding: 12)
    }
}

// MARK: - Phone Data Sheet
private struct PhoneDataSheet: View {
    enum Kind {
        case contacts
        case conversations
        case callLog

        var title: String {
            switch self {
            case .contacts: return "Contacts"
            case .conversations: return "Conversations"
            case .callLog: return "Call Log"
            }
        }
    }

    @EnvironmentObject private var model: ConnectedAppModel
    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.dismiss) private var dismiss
    @State private var toastMessage: String?
    @State private var callLogLimit: UInt32 = 100

    let kind: Kind
    let device: DiscoveredDevice

    var body: some View {
        NavigationStack {
            List {
                Section {
                    Text(device.name)
                        .font(.subheadline)
                        .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme))
                }

                switch kind {
                case .contacts:
                    contactsContent
                case .conversations:
                    conversationsContent
                case .callLog:
                    callLogContent
                }
            }
            .navigationTitle(kind.title)
            .background(GlassTheme.background(for: colorScheme).ignoresSafeArea())
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                #if os(iOS)
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done") { dismiss() }
                        .foregroundStyle(GlassTheme.primary(for: colorScheme))
                }
                #else
                ToolbarItem(placement: .automatic) {
                    Button("Done") { dismiss() }
                        .foregroundStyle(GlassTheme.primary(for: colorScheme))
                }
                #endif
            }
            .overlay(alignment: .bottom) {
                if let toastMessage {
                    Text(toastMessage)
                        .font(.footnote.weight(.semibold))
                        .foregroundStyle(.white)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 10)
                        .background(.black.opacity(0.82), in: Capsule())
                        .padding(.bottom, 24)
                        .transition(.move(edge: .bottom).combined(with: .opacity))
                }
            }
            .animation(.easeInOut(duration: 0.2), value: toastMessage)
        }
    }

    @ViewBuilder
    private var contactsContent: some View {
        if model.contacts.isEmpty {
            emptyState("Waiting for contacts from \(device.name).")
        } else {
            Section("Contacts") {
                ForEach(model.contacts, id: \.id) { contact in
                    Button {
                        copyPrimaryPhoneNumber(from: contact)
                    } label: {
                        VStack(alignment: .leading, spacing: 4) {
                            Text(contact.name)
                                .font(.body)
                                .foregroundStyle(GlassTheme.onSurface(for: colorScheme))
                            if !contact.phoneNumbers.isEmpty {
                                Text(contact.phoneNumbers.map(\.number).joined(separator: ", "))
                                    .font(.caption)
                                    .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme))
                            }
                            if !contact.emails.isEmpty {
                                Text(contact.emails.joined(separator: ", "))
                                    .font(.caption)
                                    .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme))
                            }
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
            }
        }
    }

    @ViewBuilder
    private var conversationsContent: some View {
        if model.conversations.isEmpty {
            emptyState("Waiting for conversations from \(device.name).")
        } else {
            Section("Conversations") {
                ForEach(model.conversations, id: \.id) { conversation in
                    NavigationLink {
                        ConversationHistoryView(device: device, conversation: conversation)
                            .environmentObject(model)
                    } label: {
                        VStack(alignment: .leading, spacing: 4) {
                            Text(conversationTitle(conversation))
                                .foregroundStyle(GlassTheme.onSurface(for: colorScheme))
                            if let last = conversation.lastMessage, !last.isEmpty {
                                Text(last)
                                    .font(.caption)
                                    .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme))
                                    .lineLimit(2)
                            }
                        }
                    }
                    .buttonStyle(.plain)
                }
            }
        }
    }

    @ViewBuilder
    private var callLogContent: some View {
        if model.callLog.isEmpty {
            emptyState("Waiting for call log from \(device.name).")
        } else {
            Section("Recent Calls") {
                ForEach(model.callLog, id: \.id) { entry in
                    VStack(alignment: .leading, spacing: 4) {
                        Text(entry.contactName ?? entry.number)
                            .font(.body)
                            .foregroundStyle(GlassTheme.onSurface(for: colorScheme))
                        Text("\(String(describing: entry.callType).capitalized) • \(formatDuration(entry.duration))")
                            .font(.caption)
                            .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme))
                        Text(formatTimestamp(entry.timestamp))
                            .font(.caption2)
                            .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme))
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .contentShape(Rectangle())
                    .onTapGesture {
                        copyCallLogNumber(entry.number)
                    }
                    .onAppear {
                        if entry.id == model.callLog.last?.id,
                           model.callLog.count >= Int(callLogLimit) {
                            callLogLimit += 100
                            model.requestCallLog(from: device, limit: callLogLimit, reset: false)
                        }
                    }
                }
            }
        }
    }

    private func copyPrimaryPhoneNumber(from contact: FfiContact) {
        var parts = [contact.name]
        parts.append(contentsOf: contact.phoneNumbers.map(\.number))
        parts.append(contentsOf: contact.emails)

        let value = parts
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
            .joined(separator: ", ")

        guard !value.isEmpty else {
            showToast("No contact details")
            return
        }

        #if canImport(UIKit)
        UIPasteboard.general.string = value
        showToast("Copied contact")
        #else
        showToast("Clipboard unavailable")
        #endif
    }

    private func copyCallLogNumber(_ number: String) {
        let trimmed = number.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            showToast("No phone number")
            return
        }

        #if canImport(UIKit)
        UIPasteboard.general.string = trimmed
        showToast("Copied \(trimmed)")
        #else
        showToast("Clipboard unavailable")
        #endif
    }

    private func showToast(_ message: String) {
        toastMessage = message
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.8) { [message] in
            if toastMessage == message {
                toastMessage = nil
            }
        }
    }

    private func emptyState(_ message: String) -> some View {
        Section {
            HStack(spacing: 10) {
                ProgressView()
                    .tint(GlassTheme.primary(for: colorScheme))
                Text(message)
                    .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme))
            }
        }
    }

    private func conversationTitle(_ conversation: FfiConversation) -> String {
        if !conversation.contactNames.isEmpty {
            return conversation.contactNames.joined(separator: ", ")
        }
        if !conversation.addresses.isEmpty {
            return conversation.addresses.joined(separator: ", ")
        }
        return conversation.id
    }

    private func formatDuration(_ seconds: UInt32) -> String {
        let minutes = seconds / 60
        let remainder = seconds % 60
        return minutes > 0 ? "\(minutes)m \(remainder)s" : "\(remainder)s"
    }

    private func formatTimestamp(_ timestamp: UInt64) -> String {
        let divisor: Double = timestamp > 10_000_000_000 ? 1000 : 1
        let date = Date(timeIntervalSince1970: TimeInterval(timestamp) / divisor)
        return date.formatted(date: .abbreviated, time: .shortened)
    }
}

// MARK: - Conversation History View
private struct ConversationHistoryView: View {
    @EnvironmentObject private var model: ConnectedAppModel
    @Environment(\.colorScheme) private var colorScheme

    let device: DiscoveredDevice
    let conversation: FfiConversation

    var body: some View {
        ScrollViewReader { proxy in
            List {
                if model.selectedConversationThreadId != conversation.id || model.messages.isEmpty {
                    Section {
                        HStack(spacing: 10) {
                            ProgressView()
                                .tint(GlassTheme.primary(for: colorScheme))
                            Text("Loading conversation history.")
                                .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme))
                        }
                    }
                }

                Section("Messages") {
                    ForEach(model.messages, id: \.id) { message in
                        VStack(alignment: .leading, spacing: 4) {
                            Text(message.isOutgoing ? "Me" : (message.contactName ?? message.address))
                                .font(.caption)
                                .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme))
                            Text(message.body)
                                .font(.body)
                                .foregroundStyle(GlassTheme.onSurface(for: colorScheme))
                            Text(formatTimestamp(message.timestamp))
                                .font(.caption2)
                                .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme))
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .id(message.id)
                    }
                }
            }
            .navigationTitle(conversationTitle(conversation))
            .background(GlassTheme.background(for: colorScheme).ignoresSafeArea())
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .onAppear {
                if model.selectedConversationThreadId != conversation.id {
                    model.requestMessages(for: conversation, from: device)
                } else {
                    scrollToLatestMessage(proxy)
                }
            }
            .onChangeCompat(of: model.messages.count) { _ in
                scrollToLatestMessage(proxy)
            }
        }
    }

    private func conversationTitle(_ conversation: FfiConversation) -> String {
        if !conversation.contactNames.isEmpty {
            return conversation.contactNames.joined(separator: ", ")
        }
        if !conversation.addresses.isEmpty {
            return conversation.addresses.joined(separator: ", ")
        }
        return conversation.id
    }

    private func formatTimestamp(_ timestamp: UInt64) -> String {
        let divisor: Double = timestamp > 10_000_000_000 ? 1000 : 1
        let date = Date(timeIntervalSince1970: TimeInterval(timestamp) / divisor)
        return date.formatted(date: .abbreviated, time: .shortened)
    }

    private func scrollToLatestMessage(_ proxy: ScrollViewProxy) {
        guard model.selectedConversationThreadId == conversation.id,
              let latestMessageId = model.messages.last?.id else { return }

        DispatchQueue.main.async {
            withAnimation(.easeOut(duration: 0.2)) {
                proxy.scrollTo(latestMessageId, anchor: .bottom)
            }
        }
    }
}

// MARK: - Glass Button Styles
extension View {
    func glassButton() -> some View {
        self
            .buttonStyle(.bordered)
            .clipShape(Capsule())
    }

    func glassButtonProminent() -> some View {
        self
            .buttonStyle(.borderedProminent)
            .tint(.black)
            .clipShape(Capsule())
    }

    func glassButtonDestructive() -> some View {
        self
            .buttonStyle(.bordered)
            .tint(.red)
            .clipShape(Capsule())
    }
}
