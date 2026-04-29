import Foundation
#if canImport(UIKit)
import UIKit
#endif

final class IOSFilesystemProvider: FilesystemProviderCallback, @unchecked Sendable {
    private let fileManager = FileManager.default
    private let rootURL: URL
    private let rootPath: String

    init(rootURL: URL) {
        let standardized = rootURL.standardizedFileURL
        self.rootURL = standardized
        self.rootPath = standardized.path
    }

    func listDir(path: String) throws -> [FfiFsEntry] {
        let directoryURL = try resolve(path: path)

        var isDirectory = ObjCBool(false)
        guard fileManager.fileExists(atPath: directoryURL.path, isDirectory: &isDirectory), isDirectory.boolValue else {
            throw filesystemError("Not a directory: \(path)")
        }

        let children = try fileManager.contentsOfDirectory(
            at: directoryURL,
            includingPropertiesForKeys: [.isDirectoryKey, .isSymbolicLinkKey, .contentModificationDateKey, .fileSizeKey],
            options: [.skipsHiddenFiles]
        )

        return try children
            .map { try makeEntry(for: $0) }
            .sorted { lhs, rhs in
                if lhs.entryType == rhs.entryType {
                    return lhs.name.localizedCaseInsensitiveCompare(rhs.name) == .orderedAscending
                }
                if lhs.entryType == .directory {
                    return true
                }
                if rhs.entryType == .directory {
                    return false
                }
                return lhs.name.localizedCaseInsensitiveCompare(rhs.name) == .orderedAscending
            }
    }

    func readFile(path: String, offset: UInt64, size: UInt64) throws -> Data {
        let fileURL = try resolve(path: path)
        var isDirectory = ObjCBool(false)
        guard fileManager.fileExists(atPath: fileURL.path, isDirectory: &isDirectory), !isDirectory.boolValue else {
            throw filesystemError("Not a file: \(path)")
        }

        let maxChunkSize: UInt64 = 4 * 1024 * 1024
        let desired = min(size, maxChunkSize)
        let count = Int(desired)

        do {
            let handle = try FileHandle(forReadingFrom: fileURL)
            defer {
                try? handle.close()
            }
            try handle.seek(toOffset: offset)
            return try handle.read(upToCount: count) ?? Data()
        } catch {
            throw filesystemError("Failed to read file: \(error.localizedDescription)")
        }
    }

    func writeFile(path: String, offset: UInt64, data: Data) throws -> UInt64 {
        let fileURL = try resolve(path: path)

        do {
            try fileManager.createDirectory(at: fileURL.deletingLastPathComponent(), withIntermediateDirectories: true)
            if !fileManager.fileExists(atPath: fileURL.path) {
                fileManager.createFile(atPath: fileURL.path, contents: nil)
            }

            let handle = try FileHandle(forUpdating: fileURL)
            defer {
                try? handle.close()
            }
            try handle.seek(toOffset: offset)
            try handle.write(contentsOf: data)
            return UInt64(data.count)
        } catch {
            throw filesystemError("Failed to write file: \(error.localizedDescription)")
        }
    }

    func getMetadata(path: String) throws -> FfiFsEntry {
        let itemURL = try resolve(path: path)
        return try makeEntry(for: itemURL)
    }

    func getThumbnail(path: String) throws -> Data {
        let fileURL = try resolve(path: path)
        guard isImage(url: fileURL) else {
            return Data()
        }

#if canImport(UIKit)
        guard let image = UIImage(contentsOfFile: fileURL.path) else {
            return Data()
        }

        let maxSide: CGFloat = 128
        let width = image.size.width
        let height = image.size.height
        guard width > 0, height > 0 else {
            return Data()
        }

        let ratio = min(maxSide / width, maxSide / height, 1.0)
        let targetSize = CGSize(width: width * ratio, height: height * ratio)
        let renderer = UIGraphicsImageRenderer(size: targetSize)
        let thumbnail = renderer.image { _ in
            image.draw(in: CGRect(origin: .zero, size: targetSize))
        }

        return thumbnail.jpegData(compressionQuality: 0.78) ?? Data()
#else
        return Data()
#endif
    }

    private func resolve(path: String) throws -> URL {
        let components = path
            .split(separator: "/")
            .map(String.init)
            .filter { !$0.isEmpty && $0 != "." && $0 != ".." }

        let resolved = components.reduce(rootURL) { current, part in
            current.appendingPathComponent(part)
        }.standardizedFileURL

        let candidatePath = resolved.path
        if candidatePath == rootPath || candidatePath.hasPrefix(rootPath + "/") {
            return resolved
        }

        throw filesystemError("Path escapes shared root: \(path)")
    }

    private func makeEntry(for itemURL: URL) throws -> FfiFsEntry {
        let attributes = try fileManager.attributesOfItem(atPath: itemURL.path)
        let itemType = attributes[.type] as? FileAttributeType

        let entryType: FfiFsEntryType
        switch itemType {
        case .typeDirectory:
            entryType = .directory
        case .typeSymbolicLink:
            entryType = .symlink
        case .typeRegular:
            entryType = .file
        default:
            entryType = .unknown
        }

        let size = (attributes[.size] as? NSNumber)?.uint64Value ?? 0
        let modified = (attributes[.modificationDate] as? Date).map { UInt64($0.timeIntervalSince1970) }

        return FfiFsEntry(
            name: itemURL.lastPathComponent,
            path: remotePath(for: itemURL),
            entryType: entryType,
            size: size,
            modified: modified
        )
    }

    private func remotePath(for url: URL) -> String {
        let standardized = url.standardizedFileURL.path
        if standardized == rootPath {
            return "/"
        }

        let rel = String(standardized.dropFirst(rootPath.count))
        if rel.hasPrefix("/") {
            return rel
        }
        return "/" + rel
    }

    private func isImage(url: URL) -> Bool {
        let ext = url.pathExtension.lowercased()
        return ["jpg", "jpeg", "png", "gif", "webp", "heic", "heif", "bmp", "tiff"].contains(ext)
    }

    private func filesystemError(_ message: String) -> FilesystemError {
        .Generic(msg: message)
    }
}
