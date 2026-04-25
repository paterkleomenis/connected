package com.connected.app.sync

import android.content.ContentUris
import android.content.Context
import android.database.Cursor
import android.net.Uri
import android.os.Environment
import android.provider.DocumentsContract
import android.provider.MediaStore
import android.util.Log
import java.io.File
import androidx.core.net.toUri

/**
 * Utility to resolve content URIs to real file paths when possible.
 * This avoids unnecessary temp file duplication for local files.
 */
object PathResolver {

    /**
     * Attempts to resolve a content URI to a real file path.
     * Returns null if the URI cannot be resolved to a local file path
     * (e.g., cloud storage, remote content).
     */
    fun resolveRealPath(context: Context, uri: Uri): String? {
        return try {
            when {
                // DocumentProvider
                DocumentsContract.isDocumentUri(context, uri) -> {
                    when {
                        // ExternalStorageProvider
                        isExternalStorageDocument(uri) -> {
                            val docId = DocumentsContract.getDocumentId(uri)
                            val split = docId.split(":")
                            val type = split[0]

                            if ("primary".equals(type, ignoreCase = true)) {
                                if (split.size > 1) {
                                    "${Environment.getExternalStorageDirectory()}/${split[1]}"
                                } else {
                                    Environment.getExternalStorageDirectory().absolutePath
                                }
                            } else {
                                // Secondary storage (SD card) - try to resolve
                                if (split.size > 1) {
                                    val sdCardPath = getSdCardPath(context, type)
                                    if (sdCardPath != null) {
                                        "$sdCardPath/${split[1]}"
                                    } else {
                                        null
                                    }
                                } else {
                                    null
                                }
                            }
                        }

                        // DownloadsProvider
                        isDownloadsDocument(uri) -> {
                            val id = DocumentsContract.getDocumentId(uri)

                            // Try numeric ID first
                            if (id.startsWith("raw:")) {
                                // Raw path
                                id.substring(4)
                            } else if (id.all { it.isDigit() }) {
                                val contentUri = ContentUris.withAppendedId(
                                    "content://downloads/public_downloads".toUri(),
                                    id.toLong()
                                )
                                getDataColumn(context, contentUri, null, null)
                            } else {
                                null
                            }
                        }

                        // MediaProvider
                        isMediaDocument(uri) -> {
                            val docId = DocumentsContract.getDocumentId(uri)
                            val split = docId.split(":")
                            val type = split[0]

                            val contentUri = when (type) {
                                "image" -> MediaStore.Images.Media.EXTERNAL_CONTENT_URI
                                "video" -> MediaStore.Video.Media.EXTERNAL_CONTENT_URI
                                "audio" -> MediaStore.Audio.Media.EXTERNAL_CONTENT_URI
                                else -> null
                            }

                            if (contentUri != null && split.size > 1) {
                                val selection = "_id=?"
                                val selectionArgs = arrayOf(split[1])
                                getDataColumn(context, contentUri, selection, selectionArgs)
                            } else {
                                null
                            }
                        }

                        else -> null
                    }
                }

                // MediaStore (not document URI)
                "content".equals(uri.scheme, ignoreCase = true) -> {
                    // Try to get from MediaStore
                    getDataColumn(context, uri, null, null)
                }

                // File URI
                "file".equals(uri.scheme, ignoreCase = true) -> {
                    uri.path
                }

                else -> null
            }
        } catch (e: Exception) {
            Log.w("PathResolver", "Failed to resolve path for URI: $uri", e)
            null
        }
    }

    /**
     * Get the value of the _data column for this Uri. This is useful for MediaStore URIs.
     */
    private fun getDataColumn(
        context: Context,
        uri: Uri,
        selection: String?,
        selectionArgs: Array<String>?
    ): String? {
        var cursor: Cursor? = null
        val column = "_data"
        val projection = arrayOf(column)

        try {
            cursor = context.contentResolver.query(uri, projection, selection, selectionArgs, null)
            if (cursor != null && cursor.moveToFirst()) {
                val columnIndex = cursor.getColumnIndexOrThrow(column)
                return cursor.getString(columnIndex)
            }
        } catch (e: Exception) {
            Log.w("PathResolver", "Failed to query data column for URI: $uri", e)
        } finally {
            cursor?.close()
        }
        return null
    }

    /**
     * Check if the URI is an ExternalStorageDocument
     */
    private fun isExternalStorageDocument(uri: Uri): Boolean {
        return "com.android.externalstorage.documents" == uri.authority
    }

    /**
     * Check if the URI is a DownloadsDocument
     */
    private fun isDownloadsDocument(uri: Uri): Boolean {
        return "com.android.providers.downloads.documents" == uri.authority
    }

    /**
     * Check if the URI is a MediaDocument
     */
    private fun isMediaDocument(uri: Uri): Boolean {
        return "com.android.providers.media.documents" == uri.authority
    }

    /**
     * Try to get SD card path from context
     */
    private fun getSdCardPath(context: Context, volumeId: String): String? {
        return try {
            val extDirs = context.getExternalFilesDirs(null)
            for (dir in extDirs) {
                if (dir != null && dir.absolutePath.contains(volumeId, ignoreCase = true)) {
                    // Go up from Android/data/... to root of SD card
                    var current: File? = dir
                    while (current != null) {
                        val parent = current.parentFile
                        if (parent != null && parent.name == "Android") {
                            return current.absolutePath
                        }
                        current = parent
                    }
                }
            }
            null
        } catch (e: Exception) {
            Log.w("PathResolver", "Failed to get SD card path", e)
            null
        }
    }

    /**
     * Check if a file path is accessible (exists and can be read)
     */
    fun isFileAccessible(path: String): Boolean {
        return try {
            val file = File(path)
            file.exists() && file.canRead()
        } catch (_: Exception) {
            false
        }
    }
}
