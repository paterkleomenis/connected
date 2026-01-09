package com.connected.app

import android.content.Context
import android.net.Uri
import androidx.documentfile.provider.DocumentFile
import uniffi.connected_ffi.*
import java.io.File
import java.io.FileNotFoundException

class AndroidFilesystemProvider(private val context: Context, private val rootUri: Uri) : FilesystemProviderCallback {

    private val isRawFile = rootUri.scheme == "file"
    private val rootFile = if (isRawFile) File(rootUri.path!!) else null

    private fun resolveDocumentFile(path: String): DocumentFile? {
        if (isRawFile) return null

        val root = DocumentFile.fromTreeUri(context, rootUri) ?: return null
        if (path == "/" || path.isEmpty()) return root

        var current = root
        val parts = path.trim('/').split('/')
        for (part in parts) {
            if (part.isEmpty()) continue
            current = current.findFile(part) ?: return null
        }
        return current
    }

    private fun resolveRawFile(path: String): File? {
        if (!isRawFile) return null
        if (path == "/" || path.isEmpty()) return rootFile

        // Prevent directory traversal attacks if path contains ".." (though Rust core should handle this)
        // We trust the path provided by resolvePath logic from core which usually cleans paths.
        // But for safety:
        val safePath = path.trim('/').split('/').filter { it != ".." && it.isNotEmpty() }.joinToString("/")
        return File(rootFile, safePath)
    }

    override fun listDir(path: String): List<FfiFsEntry> {
        if (isRawFile) {
            val file = resolveRawFile(path) ?: throw FilesystemException.Generic("Path not found: $path")
            if (!file.exists()) throw FilesystemException.Generic("Path not found: $path")
            if (!file.isDirectory) throw FilesystemException.Generic("Not a directory: $path")

            return file.listFiles()?.map { f ->
                FfiFsEntry(
                    name = f.name,
                    path = if (path == "/") "/${f.name}" else "$path/${f.name}",
                    entryType = if (f.isDirectory) FfiFsEntryType.DIRECTORY else FfiFsEntryType.FILE,
                    size = f.length().toULong(),
                    modified = (f.lastModified() / 1000).toULong()
                )
            } ?: emptyList()
        } else {
            val dir = resolveDocumentFile(path) ?: throw FilesystemException.Generic("Path not found: $path")
            if (!dir.isDirectory) throw FilesystemException.Generic("Not a directory: $path")

            return dir.listFiles().map { file ->
                FfiFsEntry(
                    name = file.name ?: "unknown",
                    path = if (path == "/") "/${file.name}" else "$path/${file.name}",
                    entryType = if (file.isDirectory) FfiFsEntryType.DIRECTORY else FfiFsEntryType.FILE,
                    size = file.length().toULong(),
                    modified = (file.lastModified() / 1000).toULong()
                )
            }
        }
    }

    override fun readFile(path: String, offset: ULong, size: ULong): ByteArray {
        if (isRawFile) {
            val file = resolveRawFile(path) ?: throw FilesystemException.Generic("File not found: $path")
            if (!file.exists() || !file.isFile) throw FilesystemException.Generic("Not a file: $path")

            try {
                file.inputStream().use { input ->
                    input.skip(offset.toLong())
                    val buffer = ByteArray(size.toInt())
                    val read = input.read(buffer)
                    if (read == -1) return ByteArray(0)
                    return if (read < size.toInt()) buffer.copyOf(read) else buffer
                }
            } catch (e: Exception) {
                throw FilesystemException.Generic("Read failed: ${e.message}")
            }
        } else {
            val file = resolveDocumentFile(path) ?: throw FilesystemException.Generic("File not found: $path")
            if (!file.isFile) throw FilesystemException.Generic("Not a file: $path")

            context.contentResolver.openInputStream(file.uri)?.use { input ->
                input.skip(offset.toLong())
                val buffer = ByteArray(size.toInt())
                val read = input.read(buffer)
                if (read == -1) return ByteArray(0)
                return if (read < size.toInt()) buffer.copyOf(read) else buffer
            } ?: throw FilesystemException.Generic("Could not open file: $path")
        }
    }

    override fun writeFile(path: String, offset: ULong, data: ByteArray): ULong {
        // Simple implementation
        if (offset > 0u) throw FilesystemException.Generic("Random write access not fully supported yet")

        val parts = path.trim('/').split('/')
        if (parts.isEmpty()) throw FilesystemException.Generic("Invalid path")
        val filename = parts.last()
        val parentPath = if (parts.size > 1) "/" + parts.dropLast(1).joinToString("/") else "/"

        if (isRawFile) {
            val parent = resolveRawFile(parentPath) ?: throw FilesystemException.Generic("Parent not found")
            if (!parent.exists()) throw FilesystemException.Generic("Parent directory does not exist")

            val file = File(parent, filename)
            try {
                file.outputStream().use { it.write(data) }
                return data.size.toULong()
            } catch (e: Exception) {
                throw FilesystemException.Generic("Write failed: ${e.message}")
            }
        } else {
            val parentDir = resolveDocumentFile(parentPath) ?: throw FilesystemException.Generic("Parent not found")

            // Overwrite
            parentDir.findFile(filename)?.delete()
            val newFile = parentDir.createFile("*/*", filename)
                ?: throw FilesystemException.Generic("Could not create file: $filename")

            context.contentResolver.openOutputStream(newFile.uri)?.use { output ->
                output.write(data)
            } ?: throw FilesystemException.Generic("Could not open output stream")

            return data.size.toULong()
        }
    }

    override fun getMetadata(path: String): FfiFsEntry {
        if (isRawFile) {
            val file = resolveRawFile(path) ?: throw FilesystemException.Generic("File not found: $path")
            if (!file.exists()) throw FilesystemException.Generic("File not found: $path")
            return FfiFsEntry(
                name = file.name,
                path = path,
                entryType = if (file.isDirectory) FfiFsEntryType.DIRECTORY else FfiFsEntryType.FILE,
                size = file.length().toULong(),
                modified = (file.lastModified() / 1000).toULong()
            )
        } else {
            val file = resolveDocumentFile(path) ?: throw FilesystemException.Generic("File not found: $path")
            return FfiFsEntry(
                name = file.name ?: "unknown",
                path = path,
                entryType = if (file.isDirectory) FfiFsEntryType.DIRECTORY else FfiFsEntryType.FILE,
                size = file.length().toULong(),
                modified = (file.lastModified() / 1000).toULong()
            )
        }
    }
}
