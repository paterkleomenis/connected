import SwiftUI
#if canImport(UIKit)
import UIKit
#endif

struct RemoteFilesView: View {
    @EnvironmentObject private var model: ConnectedAppModel
    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        NavigationStack {
            VStack(spacing: 10) {
                // Device info header
                HStack {
                    if let device = model.browsingDevice {
                        Label("Device: \(device.name)", systemImage: "desktopcomputer")
                            .font(.subheadline)
                            .foregroundStyle(GlassTheme.onSurface(for: colorScheme))
                    } else {
                        Text("No device selected")
                            .font(.subheadline)
                            .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme))
                    }
                    Spacer()
                }

                // Navigation controls
                HStack(spacing: 8) {
                    Button("Root") {
                        if let device = model.browsingDevice {
                            model.browseRemoteFiles(device)
                        }
                    }
                    .glassButton()
                    .disabled(model.browsingDevice == nil)

                    Button("Up") {
                        model.browseParentDirectory()
                    }
                    .glassButton()
                    .disabled(model.currentRemotePath == "/")

                    Spacer()

                    Text(model.currentRemotePath)
                        .font(.caption)
                        .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme))
                        .lineLimit(1)
                }

                // Download progress
                if let progress = model.browserDownloadProgress {
                    GlassDownloadProgressCard(progress: progress)
                }

                // File list
                if model.remoteFiles.isEmpty {
                    VStack(spacing: 12) {
                        Image(systemName: "folder")
                            .font(.system(size: 40))
                            .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme).opacity(0.5))
                        Text("No entries")
                            .font(.headline)
                            .foregroundStyle(GlassTheme.onSurface(for: colorScheme))
                        Text("Open a remote path to browse files.")
                            .font(.footnote)
                            .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme))
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 40)
                } else {
                    List(model.remoteFiles, id: \.path) { entry in
                        GlassFileRow(entry: entry, model: model)
                    }
                    #if os(iOS)
                    .listStyle(.insetGrouped)
                    #else
                    .listStyle(.inset)
                    #endif
                }
            }
            .padding()
            .glassBackground()
            .navigationTitle("Remote Files")
            .glassNavigationBar()
            .toolbar {
                #if os(iOS)
                ToolbarItem(placement: .topBarLeading) {
                    Button("Back") {
                        model.closeRemoteBrowser()
                    }
                    .foregroundStyle(GlassTheme.primary(for: colorScheme))
                }
                #else
                ToolbarItem(placement: .automatic) {
                    Button("Back") {
                        model.closeRemoteBrowser()
                    }
                    .foregroundStyle(GlassTheme.primary(for: colorScheme))
                }
                #endif
            }
        }
    }
}

// MARK: - Glass Download Progress Card
private struct GlassDownloadProgressCard: View {
    let progress: ConnectedAppModel.BrowserDownloadProgressState

    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text(progress.isFolder ? "Downloading folder..." : "Downloading...")
                    .font(.subheadline)
                    .foregroundStyle(GlassTheme.onSurface(for: colorScheme))
                Spacer()
                Text("\(Int(progress.fractionCompleted * 100))%")
                    .font(.subheadline)
                    .foregroundStyle(GlassTheme.onSurface(for: colorScheme))
            }

            Text(progress.currentFile)
                .font(.caption)
                .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme))
                .lineLimit(1)

            ProgressView(value: progress.fractionCompleted)

            HStack {
                Text(ByteCountFormatter.string(fromByteCount: Int64(progress.bytesDownloaded), countStyle: .file))
                    .font(.caption2)
                    .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme))
                Spacer()
                Text(ByteCountFormatter.string(fromByteCount: Int64(progress.totalBytes), countStyle: .file))
                    .font(.caption2)
                    .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme))
            }
        }
        .glassCard(padding: 12)
    }
}

// MARK: - Glass File Row
private struct GlassFileRow: View {
    let entry: FfiFsEntry
    let model: ConnectedAppModel

    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        HStack(spacing: 10) {
            // Thumbnail or icon
            thumbnailOrIcon

            // File info
            VStack(alignment: .leading, spacing: 2) {
                Text(entry.name)
                    .font(.body)
                    .foregroundStyle(GlassTheme.onSurface(for: colorScheme))
                    .lineLimit(1)

                if entry.entryType != .directory {
                    Text(ByteCountFormatter.string(fromByteCount: Int64(entry.size), countStyle: .file))
                        .font(.caption)
                        .foregroundStyle(GlassTheme.onSurfaceVariant(for: colorScheme))
                }
            }

            Spacer()

            // Action button
            if entry.entryType == .directory {
                Button("Open") {
                    model.openRemoteEntry(entry)
                }
                .glassButton()
            } else {
                Button("Download") {
                    model.downloadRemoteEntry(entry)
                }
                .glassButtonProminent()
            }
        }
        .onAppear {
            if shouldRequestThumbnail(for: entry) {
                model.getThumbnail(path: entry.path)
            }
        }
    }

    @ViewBuilder
    private var thumbnailOrIcon: some View {
        #if canImport(UIKit)
        if let data = model.thumbnailDataByPath[entry.path],
           let image = UIImage(data: data) {
            Image(uiImage: image)
                .resizable()
                .scaledToFill()
                .frame(width: 36, height: 36)
                .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        } else {
            fileIcon
        }
        #else
        fileIcon
        #endif
    }

    private var fileIcon: some View {
        Image(systemName: icon(for: entry.entryType))
            .font(.body)
            .foregroundStyle(
                entry.entryType == .directory
                    ? GlassTheme.primary(for: colorScheme)
                    : GlassTheme.onSurfaceVariant(for: colorScheme)
            )
            .frame(width: 36, height: 36)
            .background(
                (entry.entryType == .directory
                    ? GlassTheme.primary(for: colorScheme)
                    : GlassTheme.onSurfaceVariant(for: colorScheme)
                ).opacity(0.1)
            )
            .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
    }

    private func icon(for entryType: FfiFsEntryType) -> String {
        switch entryType {
        case .directory:
            return "folder.fill"
        case .file:
            return "doc"
        case .symlink:
            return "link"
        case .unknown:
            return "questionmark.folder"
        }
    }

    private func shouldRequestThumbnail(for entry: FfiFsEntry) -> Bool {
        guard entry.entryType == .file else { return false }
        let ext = URL(fileURLWithPath: entry.name).pathExtension.lowercased()
        return [
            "jpg", "jpeg", "png", "gif", "webp", "heic", "heif", "bmp", "tif", "tiff",
            "mp4", "mov", "m4v", "avi", "mkv", "webm"
        ].contains(ext)
    }
}
