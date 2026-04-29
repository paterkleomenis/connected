import SwiftUI
#if canImport(UIKit)
import UIKit
#endif

struct RemoteFilesView: View {
    @EnvironmentObject private var model: ConnectedAppModel

    var body: some View {
        NavigationStack {
            VStack(spacing: 10) {
                HStack {
                    if let device = model.browsingDevice {
                        Text("Device: \(device.name)")
                            .font(.subheadline)
                    } else {
                        Text("No device selected")
                            .font(.subheadline)
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                }

                HStack(spacing: 8) {
                    Button("Root") {
                        if let device = model.browsingDevice {
                            model.browseRemoteFiles(device)
                        }
                    }
                    .buttonStyle(.bordered)
                    .disabled(model.browsingDevice == nil)

                    Button("Up") {
                        model.browseParentDirectory()
                    }
                    .buttonStyle(.bordered)
                    .disabled(model.currentRemotePath == "/")

                    Spacer()

                    Text(model.currentRemotePath)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }

                if let progress = model.browserDownloadProgress {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("Downloading \(progress.currentFile)")
                            .font(.caption)
                        ProgressView(value: progress.fractionCompleted)
                        Text(progressText(progress))
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                    }
                }

                if model.remoteFiles.isEmpty {
                    VStack(spacing: 8) {
                        Image(systemName: "folder")
                            .font(.title2)
                            .foregroundStyle(.secondary)
                        Text("No entries")
                            .font(.headline)
                        Text("Open a remote path to browse files.")
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 24)
                } else {
                    List(model.remoteFiles, id: \.path) { entry in
                        HStack(spacing: 10) {
                            thumbnailOrIcon(for: entry)
                            VStack(alignment: .leading) {
                                Text(entry.name)
                                    .lineLimit(1)
                                if entry.entryType != .directory {
                                    Text(ByteCountFormatter.string(fromByteCount: Int64(entry.size), countStyle: .file))
                                        .font(.caption)
                                        .foregroundStyle(.secondary)
                                }
                            }

                            Spacer()

                            if entry.entryType == .directory {
                                Button("Open") {
                                    model.openRemoteEntry(entry)
                                }
                                .buttonStyle(.bordered)
                            } else {
                                Button("Download") {
                                    model.downloadRemoteEntry(entry)
                                }
                                .buttonStyle(.borderedProminent)
                            }
                        }
                        .onAppear {
                            if shouldRequestThumbnail(for: entry) {
                                model.getThumbnail(path: entry.path)
                            }
                        }
                    }
#if os(iOS)
                    .listStyle(.insetGrouped)
#else
                    .listStyle(.inset)
#endif
                }
            }
            .padding()
            .navigationTitle("Remote Files")
            .toolbar {
#if os(iOS)
                ToolbarItem(placement: .topBarLeading) {
                    Button("Back") {
                        model.closeRemoteBrowser()
                    }
                }
#else
                ToolbarItem(placement: .automatic) {
                    Button("Back") {
                        model.closeRemoteBrowser()
                    }
                }
#endif
            }
        }
    }

    private func icon(for entryType: FfiFsEntryType) -> String {
        switch entryType {
        case .directory:
            return "folder"
        case .file:
            return "doc"
        case .symlink:
            return "link"
        case .unknown:
            return "questionmark.folder"
        }
    }

    private func progressText(_ progress: ConnectedAppModel.BrowserDownloadProgressState) -> String {
        let downloaded = ByteCountFormatter.string(fromByteCount: Int64(progress.bytesDownloaded), countStyle: .file)
        let total = ByteCountFormatter.string(fromByteCount: Int64(progress.totalBytes), countStyle: .file)
        return "\(downloaded) / \(total)"
    }

    @ViewBuilder
    private func thumbnailOrIcon(for entry: FfiFsEntry) -> some View {
#if canImport(UIKit)
        if let data = model.thumbnailDataByPath[entry.path],
           let image = UIImage(data: data) {
            Image(uiImage: image)
                .resizable()
                .scaledToFill()
                .frame(width: 32, height: 32)
                .clipShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
        } else {
            Image(systemName: icon(for: entry.entryType))
                .frame(width: 32, height: 32)
                .foregroundStyle(.secondary)
        }
#else
        Image(systemName: icon(for: entry.entryType))
            .frame(width: 32, height: 32)
            .foregroundStyle(.secondary)
#endif
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
