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
    @State private var clipboardDraft = ""
    @State private var showingFileImporter = false
    @State private var selectedFileTarget: DiscoveredDevice?
    @State private var phoneDataRoute: PhoneDataRoute?

    var body: some View {
        NavigationStack {
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 16) {
                    if let info = model.infoMessage {
                        StatusCard(systemImage: "info.circle", text: info)
                    }

                    if !model.pendingShareURLs.isEmpty {
                        StatusCard(
                            systemImage: "tray.full",
                            text: "\(model.pendingShareURLs.count) shared item(s) queued"
                        )
                    }

                    DeviceSectionCard(title: "Discovery", subtitle: "Find nearby devices and refresh trust state.") {
                        HStack(spacing: 10) {
                            Button(model.isDiscoveryActive ? "Stop" : "Start") {
                                model.setDiscoveryActive(!model.isDiscoveryActive)
                            }
                            .buttonStyle(.bordered)

                            Button("Refresh") {
                                model.refreshDiscoveryNow()
                            }
                            .buttonStyle(.borderedProminent)
                        }
                    }

                    if model.devices.isEmpty {
                        DeviceSectionCard(title: "Devices", subtitle: "Waiting for nearby peers.") {
                            VStack(spacing: 8) {
                                Image(systemName: "dot.radiowaves.left.and.right")
                                    .font(.title2)
                                    .foregroundStyle(.secondary)
                                Text("No devices yet")
                                    .font(.headline)
                            }
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 12)
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
                                    onUnpair: { model.unpairDevice(device) },
                                    onForget: { model.forgetDevice(device) },
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

                    clipboardCard
                    if model.transferStatus != "Idle" {
                        transferCard
                    }
                }
                .padding(16)
            }
            .background(screenBackground.ignoresSafeArea())
            .navigationTitle("Connected")
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

    private var clipboardCard: some View {
        DeviceSectionCard(title: "Clipboard", subtitle: "Send custom text or use iOS Paste & Share.") {
            VStack(alignment: .leading, spacing: 12) {
                pasteAndShareControl

                TextField("Type text to send", text: $clipboardDraft, axis: .vertical)
                    .textFieldStyle(.roundedBorder)

                HStack(spacing: 10) {
                    Button("Use Last Received") {
                        clipboardDraft = model.clipboardContent
                    }
                    .buttonStyle(.bordered)

                    Button("Send to All") {
                        model.sendClipboardTextToAllTrusted(clipboardDraft)
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(!model.hasTrustedDevices || clipboardDraft.isEmpty)
                }

            }
        }
    }

    private var pasteAndShareControl: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Image(systemName: "doc.on.clipboard.fill")
                    .foregroundStyle(.tint)
                VStack(alignment: .leading, spacing: 2) {
                    Text("Paste & Share")
                        .font(.headline)
                    Text("Paste through iOS and send to every trusted device.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }

            PasteButton(payloadType: String.self) { strings in
                let text = strings
                    .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
                    .filter { !$0.isEmpty }
                    .joined(separator: "\n")
                model.sendClipboardTextToAllTrusted(text)
            }
            .controlSize(.large)
            .tint(.accentColor)
            .disabled(!model.hasTrustedDevices)

            if !model.hasTrustedDevices {
                Text("Pair and trust a device to enable Paste & Share.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.accentColor.opacity(model.shouldHighlightPasteAndShare ? 0.16 : 0.06))
        .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 14, style: .continuous)
                .stroke(
                    model.shouldHighlightPasteAndShare ? Color.accentColor : Color.accentColor.opacity(0.2),
                    lineWidth: model.shouldHighlightPasteAndShare ? 2 : 1
                )
        }
    }

    private var transferCard: some View {
        DeviceSectionCard(title: "Transfer", subtitle: model.transferStatus) {
            HStack(spacing: 10) {
                if model.activeTransferId != nil {
                    Button("Cancel", role: .destructive) {
                        model.cancelActiveTransfer()
                    }
                    .buttonStyle(.bordered)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
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

private struct DeviceRow: View {
    let device: DiscoveredDevice
    let isTrusted: Bool
    let isPending: Bool
    let hasPendingShare: Bool
    let isMediaControlEnabled: Bool
    let onPair: () -> Void
    let onUnpair: () -> Void
    let onForget: () -> Void
    let onSendFile: () -> Void
    let onSendPendingShare: () -> Void
    let onShareClipboard: () -> Void
    let onBrowse: () -> Void
    let onRequestContacts: () -> Void
    let onRequestConversations: () -> Void
    let onRequestCallLog: () -> Void
    let onMediaCommand: (MediaCommand) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                Image(systemName: iconName(for: device.deviceType))
                    .font(.title3)
                    .foregroundStyle(.tint)
                    .frame(width: 24)

                VStack(alignment: .leading, spacing: 3) {
                    Text(device.name)
                        .font(.body)
                        .lineLimit(1)
                    Text("\(device.ip):\(device.port)")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    statusLabel
                }

                Spacer(minLength: 8)

                if isTrusted {
                    Button("Send File") {
                        onSendFile()
                    }
                    .buttonStyle(.borderedProminent)
                    .controlSize(.small)

                    Menu {
                        if hasPendingShare {
                            Button("Send Queued Share", action: onSendPendingShare)
                        }
                        Button("Share Clipboard", action: onShareClipboard)
                        Button("Browse Files", action: onBrowse)
                        Button("Request Contacts", action: onRequestContacts)
                        Button("Request Conversations", action: onRequestConversations)
                        Button("Request Call Log", action: onRequestCallLog)
                        Button("Unpair", role: .destructive, action: onUnpair)
                        Button("Forget", role: .destructive, action: onForget)
                    } label: {
                        Image(systemName: "gearshape")
                            .frame(width: 28, height: 28)
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                } else {
                    Button("Send File") {
                        onSendFile()
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)

                    Button(isPending ? "Waiting" : "Pair") {
                        onPair()
                    }
                    .buttonStyle(.borderedProminent)
                    .controlSize(.small)
                    .disabled(isPending)
                }
            }

            if isTrusted && isMediaControlEnabled {
                MediaCommandRow(onCommand: onMediaCommand)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
    }

    @ViewBuilder
    private var statusLabel: some View {
        if isTrusted {
            Label("Trusted", systemImage: "checkmark.shield")
                .font(.caption2)
                .foregroundStyle(.tint)
        } else if isPending {
            Label("Pending", systemImage: "hourglass")
                .font(.caption2)
                .foregroundStyle(.orange)
        }
    }

    private var cardBackground: Color {
#if canImport(UIKit)
        Color(uiColor: .secondarySystemGroupedBackground)
#else
        Color.clear
#endif
    }

    private func iconName(for type: String) -> String {
        let normalized = type.lowercased()
        if normalized.contains("android") { return "iphone.gen3.radiowaves.left.and.right" }
        if normalized.contains("ios") || normalized.contains("iphone") { return "iphone" }
        if normalized.contains("ipad") { return "ipad" }
        if normalized.contains("mac") { return "laptopcomputer" }
        if normalized.contains("windows") { return "desktopcomputer" }
        if normalized.contains("linux") { return "terminal" }
        return "display"
    }
}

private struct MediaCommandRow: View {
    let onCommand: (MediaCommand) -> Void

    var body: some View {
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
        .padding(.top, 4)
    }

    private func mediaButton(systemImage: String, label: String, command: MediaCommand) -> some View {
        Button {
            onCommand(command)
        } label: {
            Image(systemName: systemImage)
                .frame(width: 32, height: 32)
        }
        .buttonStyle(.bordered)
        .controlSize(.small)
        .accessibilityLabel(label)
    }
}

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
                        .foregroundStyle(.secondary)
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
#if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
#endif
            .toolbar {
#if os(iOS)
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done") { dismiss() }
                }
#else
                ToolbarItem(placement: .automatic) {
                    Button("Done") { dismiss() }
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
                                .foregroundStyle(.primary)
                            if !contact.phoneNumbers.isEmpty {
                                Text(contact.phoneNumbers.map(\.number).joined(separator: ", "))
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                            if !contact.emails.isEmpty {
                                Text(contact.emails.joined(separator: ", "))
                                    .font(.caption)
                                .foregroundStyle(.secondary)
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
                                .foregroundStyle(.primary)
                            if let last = conversation.lastMessage, !last.isEmpty {
                                Text(last)
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
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
                        Text("\(String(describing: entry.callType).capitalized) • \(formatDuration(entry.duration))")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                        Text(formatTimestamp(entry.timestamp))
                            .font(.caption2)
                            .foregroundStyle(.secondary)
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
                Text(message)
                    .foregroundStyle(.secondary)
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

private struct ConversationHistoryView: View {
    @EnvironmentObject private var model: ConnectedAppModel

    let device: DiscoveredDevice
    let conversation: FfiConversation

    var body: some View {
        ScrollViewReader { proxy in
            List {
                if model.selectedConversationThreadId != conversation.id || model.messages.isEmpty {
                    Section {
                        HStack(spacing: 10) {
                            ProgressView()
                            Text("Loading conversation history.")
                                .foregroundStyle(.secondary)
                        }
                    }
                }

                Section("Messages") {
                    ForEach(model.messages, id: \.id) { message in
                        VStack(alignment: .leading, spacing: 4) {
                            Text(message.isOutgoing ? "Me" : (message.contactName ?? message.address))
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            Text(message.body)
                                .font(.body)
                                .foregroundStyle(.primary)
                            Text(formatTimestamp(message.timestamp))
                                .font(.caption2)
                                .foregroundStyle(.secondary)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .id(message.id)
                    }
                }
            }
            .navigationTitle(conversationTitle(conversation))
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

private struct DeviceSectionCard<Content: View>: View {
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

private struct StatusCard: View {
    let systemImage: String
    let text: String

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: systemImage)
            Text(text)
                .font(.footnote)
                .lineLimit(2)
            Spacer()
        }
        .foregroundStyle(.secondary)
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
    }

    private var cardBackground: Color {
#if canImport(UIKit)
        Color(uiColor: .secondarySystemGroupedBackground)
#else
        Color.clear
#endif
    }
}
