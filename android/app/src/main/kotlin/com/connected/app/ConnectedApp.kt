package com.connected.app

import android.content.Context
import android.util.Log
import android.net.Uri
import android.database.Cursor
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import uniffi.connected_ffi.*
import java.io.File
import java.io.InputStream
import java.io.OutputStream

class ConnectedApp(private val context: Context) {
    // State exposed to Compose
    val devices = mutableStateListOf<DiscoveredDevice>()
    val trustedDevices = mutableStateListOf<String>() // Set of trusted Device IDs
    val pendingPairing = mutableStateListOf<String>() // Set of pending Device IDs
    val transferStatus = mutableStateOf("Idle")
    val clipboardContent = mutableStateOf("")
    val pairingRequest = mutableStateOf<PairingRequest?>(null)
    val transferRequest = mutableStateOf<TransferRequest?>(null)

    data class PairingRequest(val deviceName: String, val fingerprint: String, val deviceId: String)
    data class TransferRequest(val id: String, val filename: String, val fileSize: ULong, val fromDevice: String)

    // Track when other devices unpair us
    val unpairNotification = mutableStateOf<String?>(null)

        // Store selected device for file transfer
        private var selectedDeviceForFile: DiscoveredDevice? = null
        private lateinit var downloadDir: File

        private val discoveryCallback = object : DiscoveryCallback {
            override fun onDeviceFound(device: DiscoveredDevice) {
                Log.d("ConnectedApp", "Device found: ${device.name}")

                // Deduplication and Update logic
                val existingIndex = devices.indexOfFirst { it.id == device.id }
                if (existingIndex >= 0) {
                    // Update existing device info (e.g. IP/Port might have changed)
                    devices[existingIndex] = device
                } else {
                    // Remove potential stale entry with same IP/Port but different ID
                    val staleIndex = devices.indexOfFirst { it.ip == device.ip && it.port == device.port }
                    if (staleIndex >= 0) {
                        Log.d("ConnectedApp", "Removing stale device at ${device.ip}:${device.port}")
                        devices.removeAt(staleIndex)
                    }
                    devices.add(device)
                }

                // Check trust status for new/updated device
                if (isDeviceTrusted(device)) {
                    if (!trustedDevices.contains(device.id)) {
                        trustedDevices.add(device.id)
                    }
                    // If it was pending, remove it
                    if (pendingPairing.contains(device.id)) {
                        pendingPairing.remove(device.id)
                    }

                    // Automatically pair (handshake) to confirm connection
                    try {
                        // Call FFI directly to avoid "Pairing request sent" toast
                        uniffi.connected_ffi.pairDevice(device.ip, device.port)
                    } catch (e: Exception) {
                        Log.w("ConnectedApp", "Failed to auto-connect to trusted device", e)
                    }
                }
            }

            override fun onDeviceLost(deviceId: String) {
                Log.d("ConnectedApp", "Device lost: $deviceId")
                devices.removeAll { it.id == deviceId }
            }

            override fun onError(errorMsg: String) {
                Log.e("ConnectedApp", "Discovery error: $errorMsg")
            }
        }

        private val transferCallback = object : FileTransferCallback {
            override fun onTransferRequest(transferId: String, filename: String, fileSize: ULong, fromDevice: String) {
                Log.d("ConnectedApp", "Transfer request from $fromDevice: $filename")
                transferRequest.value = TransferRequest(transferId, filename, fileSize, fromDevice)
            }

            override fun onTransferStarting(filename: String, totalSize: ULong) {
                transferStatus.value = "Starting transfer: $filename"
            }

            override fun onTransferProgress(bytesTransferred: ULong, totalSize: ULong) {
                val percent = if (totalSize > 0u) (bytesTransferred.toLong() * 100 / totalSize.toLong()) else 0
                transferStatus.value = "Transferring: $percent%"
            }

            override fun onTransferCompleted(filename: String, totalSize: ULong) {
                transferStatus.value = "Completed: $filename"
                moveToDownloads(filename)
            }

            override fun onTransferFailed(errorMsg: String) {
                transferStatus.value = "Failed: $errorMsg"
            }

            override fun onTransferCancelled() {
                transferStatus.value = "Cancelled"
            }
        }

        private val clipboardCallback = object : ClipboardCallback {
            override fun onClipboardReceived(text: String, fromDevice: String) {
                clipboardContent.value = text
                // TODO: Copy to system clipboard
            }

            override fun onClipboardSent(success: Boolean, errorMsg: String?) {
                // Log result
            }
        }

        private val pairingCallback = object : PairingCallback {
            override fun onPairingRequest(deviceName: String, fingerprint: String, deviceId: String) {
                Log.d("ConnectedApp", "Pairing request from $deviceName")
                pairingRequest.value = PairingRequest(deviceName, fingerprint, deviceId)
            }
        }

        private val unpairCallback = object : UnpairCallback {
            override fun onDeviceUnpaired(deviceId: String, deviceName: String, reason: String) {
                Log.d("ConnectedApp", "Device $deviceName unpaired us (reason: $reason)")
                // Remove from trusted devices
                trustedDevices.remove(deviceId)

                // Show notification to user
                val reasonText = when (reason) {
                    "blocked" -> "blocked you"
                    "forgotten" -> "forgot you"
                    else -> "unpaired from you"
                }
                unpairNotification.value = "$deviceName $reasonText"
            }
        }

            fun initialize() {
                try {
                    // Create a dedicated download directory in the app's private storage
                    // The core will append "downloads" to the storage path we provide
                    downloadDir = File(context.getExternalFilesDir(null), "downloads")
                    if (!downloadDir.exists()) {
                        downloadDir.mkdirs()
                    }

                    // Pass the root files directory. Core will join("downloads") to this.
                    val storagePath = context.getExternalFilesDir(null)?.absolutePath ?: ""

                    uniffi.connected_ffi.initialize(
                        "Android Device",
                        "Mobile",
                        0u.toUShort(),
                        storagePath
                    )
                    uniffi.connected_ffi.startDiscovery(discoveryCallback)
                    uniffi.connected_ffi.registerTransferCallback(transferCallback)
                    uniffi.connected_ffi.registerClipboardReceiver(clipboardCallback)
                    uniffi.connected_ffi.registerPairingCallback(pairingCallback)
                    uniffi.connected_ffi.registerUnpairCallback(unpairCallback)
                } catch (e: Exception) {
                    Log.e("ConnectedApp", "Initialization failed", e)
                }
            }
        private fun moveToDownloads(filename: String) {
            val sourceFile = File(downloadDir, filename)
            if (!sourceFile.exists()) {
                Log.e("ConnectedApp", "Source file not found: ${sourceFile.absolutePath}")
                return
            }

            try {
                val contentValues = android.content.ContentValues().apply {
                    put(android.provider.MediaStore.MediaColumns.DISPLAY_NAME, filename)
                    put(android.provider.MediaStore.MediaColumns.MIME_TYPE, getMimeType(filename))
                    if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.Q) {
                        put(android.provider.MediaStore.MediaColumns.RELATIVE_PATH, android.os.Environment.DIRECTORY_DOWNLOADS)
                        put(android.provider.MediaStore.MediaColumns.IS_PENDING, 1)
                    }
                }

                val resolver = context.contentResolver
                val uri = if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.Q) {
                     android.provider.MediaStore.Downloads.EXTERNAL_CONTENT_URI
                } else {
                     android.provider.MediaStore.Files.getContentUri("external")
                }

                val itemUri = resolver.insert(uri, contentValues)
                if (itemUri != null) {
                    resolver.openOutputStream(itemUri).use { output ->
                        sourceFile.inputStream().use { input ->
                            input.copyTo(output!!)
                        }
                    }

                    if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.Q) {
                        contentValues.clear()
                        contentValues.put(android.provider.MediaStore.MediaColumns.IS_PENDING, 0)
                        resolver.update(itemUri, contentValues, null, null)
                    }

                    // Show toast on UI thread
                    runOnMainThread {
                        android.widget.Toast.makeText(
                            context,
                            "Saved to Downloads: $filename",
                            android.widget.Toast.LENGTH_LONG
                        ).show()
                    }
                }
            } catch (e: Exception) {
                Log.e("ConnectedApp", "Failed to save to Downloads", e)
                 runOnMainThread {
                     android.widget.Toast.makeText(
                        context,
                        "Failed to save to Downloads: ${e.message}",
                        android.widget.Toast.LENGTH_LONG
                    ).show()
                 }
            }
        }

        private fun getMimeType(url: String): String {
            val ext = android.webkit.MimeTypeMap.getFileExtensionFromUrl(url)
            return if (ext != null) {
                android.webkit.MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext) ?: "*/*"
            } else {
                "*/*"
            }
        }

        private fun runOnMainThread(action: () -> Unit) {
            android.os.Handler(android.os.Looper.getMainLooper()).post(action)
        }

        fun startDiscovery() {        // Already started in initialize, but exposed if needed to restart
        try {
            uniffi.connected_ffi.startDiscovery(discoveryCallback)
        } catch (e: Exception) {
            Log.e("ConnectedApp", "Start discovery failed", e)
        }
    }

    fun pairDevice(device: DiscoveredDevice) {
        try {
            uniffi.connected_ffi.pairDevice(device.ip, device.port)
            android.widget.Toast.makeText(
                context,
                "Pairing request sent to ${device.name}",
                android.widget.Toast.LENGTH_SHORT
            ).show()
            if (!pendingPairing.contains(device.id)) {
                pendingPairing.add(device.id)
            }
        } catch (e: Exception) {
            Log.e("ConnectedApp", "Pairing failed", e)
            android.widget.Toast.makeText(context, "Pairing failed: ${e.message}", android.widget.Toast.LENGTH_SHORT)
                .show()
        }
    }

    fun unpairDevice(device: DiscoveredDevice) {
        // Unpair = disconnect and remove from UI, but keep backend trust (can auto-reconnect later)
        try {
            uniffi.connected_ffi.unpairDeviceById(device.id)
            // Remove from UI trusted list so it shows as disconnected
            trustedDevices.remove(device.id)
            // Notify the other device so they also update their UI
            // Using "unpaired" reason which preserves backend trust on their side
            try {
                uniffi.connected_ffi.sendUnpairNotification(device.ip, device.port, "unpaired")
            } catch (e: Exception) {
                Log.w("ConnectedApp", "Failed to send unpair notification: ${e.message}")
            }
            android.widget.Toast.makeText(
                context,
                "Disconnected from ${device.name}",
                android.widget.Toast.LENGTH_SHORT
            ).show()
        } catch (e: Exception) {
            Log.e("ConnectedApp", "Unpair failed", e)
        }
    }

    fun forgetDevice(device: DiscoveredDevice) {
        try {
            uniffi.connected_ffi.forgetDeviceById(device.id)
            trustedDevices.remove(device.id)
            if (pendingPairing.contains(device.id)) {
                pendingPairing.remove(device.id)
            }
            // Notify the other device that we forgot them
            try {
                uniffi.connected_ffi.sendUnpairNotification(device.ip, device.port, "forgotten")
            } catch (e: Exception) {
                Log.w("ConnectedApp", "Failed to send forget notification: ${e.message}")
            }
            getDevices()
            android.widget.Toast.makeText(
                context,
                "Forgot ${device.name} - will require re-approval to pair",
                android.widget.Toast.LENGTH_SHORT
            ).show()
        } catch (e: Exception) {
            Log.e("ConnectedApp", "Forget device failed", e)
            android.widget.Toast.makeText(context, "Forget failed: ${e.message}", android.widget.Toast.LENGTH_SHORT)
                .show()
        }
    }

    fun blockDevice(device: DiscoveredDevice) {
        // Save ip/port before removing from list
        val ip = device.ip
        val port = device.port
        try {
            uniffi.connected_ffi.blockDeviceById(device.id)
            trustedDevices.remove(device.id)
            devices.removeAll { it.id == device.id }
            if (pendingPairing.contains(device.id)) {
                pendingPairing.remove(device.id)
            }
            // Notify the other device that we blocked them
            try {
                uniffi.connected_ffi.sendUnpairNotification(ip, port, "blocked")
            } catch (e: Exception) {
                Log.w("ConnectedApp", "Failed to send block notification: ${e.message}")
            }
            android.widget.Toast.makeText(
                context,
                "Blocked ${device.name} - device can no longer connect",
                android.widget.Toast.LENGTH_SHORT
            ).show()
        } catch (e: Exception) {
            Log.e("ConnectedApp", "Block device failed", e)
            android.widget.Toast.makeText(context, "Block failed: ${e.message}", android.widget.Toast.LENGTH_SHORT)
                .show()
        }
    }

    fun isDeviceTrusted(device: DiscoveredDevice): Boolean {
        return try {
            uniffi.connected_ffi.isDeviceTrusted(device.id)
        } catch (e: Exception) {
            false
        }
    }

    fun trustDevice(request: PairingRequest) {
        try {
            // Pass device_id so is_device_trusted() can find the peer later
            uniffi.connected_ffi.trustDevice(request.fingerprint, request.deviceId, request.deviceName)
            pairingRequest.value = null

            // Send trust confirmation (NOT a new handshake) to the other device
            // This lets them know we accepted their pairing request
            val device = devices.find { it.id == request.deviceId }
            if (device != null) {
                sendTrustConfirmation(device)
                if (!trustedDevices.contains(device.id)) {
                    trustedDevices.add(device.id)
                }
            }
            getDevices()
        } catch (e: Exception) {
            Log.e("ConnectedApp", "Trust device failed", e)
        }
    }

    private fun sendTrustConfirmation(device: DiscoveredDevice) {
        try {
            uniffi.connected_ffi.sendTrustConfirmation(device.ip, device.port)
        } catch (e: Exception) {
            Log.w("ConnectedApp", "Failed to send trust confirmation: ${e.message}")
            // Don't fail the trust operation if confirmation fails
        }
    }

    fun rejectDevice(request: PairingRequest) {
        try {
            uniffi.connected_ffi.blockDevice(request.fingerprint)
            android.widget.Toast.makeText(context, "Blocked ${request.deviceName}", android.widget.Toast.LENGTH_SHORT)
                .show()
        } catch (e: Exception) {
            Log.e("ConnectedApp", "Block device failed", e)
        }
        pairingRequest.value = null
    }

    fun acceptTransfer(request: TransferRequest) {
        try {
            uniffi.connected_ffi.acceptFileTransfer(request.id)
        } catch (e: Exception) {
            Log.e("ConnectedApp", "Accept transfer failed", e)
        }
        transferRequest.value = null
    }

    fun rejectTransfer(request: TransferRequest) {
        try {
            uniffi.connected_ffi.rejectFileTransfer(request.id)
        } catch (e: Exception) {
            Log.e("ConnectedApp", "Reject transfer failed", e)
        }
        transferRequest.value = null
    }

    fun getDevices() {
        // Force refresh from core if needed
        try {
            val list = uniffi.connected_ffi.getDiscoveredDevices()
            devices.clear()
            devices.addAll(list)

            // Refresh trust status for all
            trustedDevices.clear()
            list.forEach {
                if (isDeviceTrusted(it)) {
                    trustedDevices.add(it.id)
                    // If it was pending, remove it
                    if (pendingPairing.contains(it.id)) {
                        pendingPairing.remove(it.id)
                    }
                }
            }
        } catch (e: Exception) {
            Log.e("ConnectedApp", "Get devices failed", e)
        }
    }

    fun sendFileToDevice(device: DiscoveredDevice) {
        try {
            // This will need to be handled by the Activity to open file picker
            // We'll emit a custom event to handle this in the MainActivity
            // For now, we'll add a placeholder function
            Log.d("ConnectedApp", "Attempting to send file to ${device.name}")
        } catch (e: Exception) {
            Log.e("ConnectedApp", "Send file to device failed", e)
        }
    }

    fun setSelectedDeviceForFileTransfer(device: DiscoveredDevice) {
        selectedDeviceForFile = device
        Log.d("ConnectedApp", "Selected device for file transfer: ${device.name}")
    }

    fun getSelectedDeviceForFileTransfer(): DiscoveredDevice? {
        return selectedDeviceForFile
    }

    fun sendFileToDevice(device: DiscoveredDevice, contentUri: String) {
        try {
            // Get the real file path from content URI
            val realPath = getRealPathFromUri(contentUri)
            if (realPath != null) {
                uniffi.connected_ffi.sendFile(device.ip, device.port.toUShort(), realPath)
                android.widget.Toast.makeText(
                    context,
                    "Started sending file to ${device.name}",
                    android.widget.Toast.LENGTH_SHORT
                ).show()
            } else {
                android.widget.Toast.makeText(
                    context,
                    "Could not resolve file path",
                    android.widget.Toast.LENGTH_SHORT
                ).show()
            }
        } catch (e: Exception) {
            Log.e("ConnectedApp", "Send file to device failed", e)
            android.widget.Toast.makeText(
                context,
                "Failed to send file: ${e.message}",
                android.widget.Toast.LENGTH_SHORT
            ).show()
        }
    }

    // Helper method to get real path from content URI
    private fun getRealPathFromUri(contentUri: String): String? {
        try {
            val uri = android.net.Uri.parse(contentUri)
            val cursor = context.contentResolver.query(uri, null, null, null, null)
            cursor?.use {
                val nameIndex = it.getColumnIndex(android.provider.MediaStore.Files.FileColumns.DATA)
                if (nameIndex >= 0) {
                    it.moveToFirst()
                    return it.getString(nameIndex)
                }
            }
        } catch (e: Exception) {
            Log.e("ConnectedApp", "Error getting real path from URI", e)
        }

        // Alternative method for newer Android versions
        try {
            val uri = android.net.Uri.parse(contentUri)
            val parcelFileDescriptor = context.contentResolver.openFileDescriptor(uri, "r")
            parcelFileDescriptor?.close()

            // Copy file to app's private storage temporarily
            val inputStream = context.contentResolver.openInputStream(uri)
            val fileName = getFileName(uri)
            val tempFile = File(context.cacheDir, fileName ?: "temp_file")

            inputStream?.use { input ->
                val outputStream = tempFile.outputStream()
                outputStream.use { output ->
                    input.copyTo(output)
                }
            }

            return tempFile.absolutePath
        } catch (e: Exception) {
            Log.e("ConnectedApp", "Error copying file to temp location", e)
        }

        return null
    }

    private fun getFileName(uri: android.net.Uri): String? {
        try {
            val cursor = context.contentResolver.query(uri, null, null, null, null)
            cursor?.use {
                val nameIndex = it.getColumnIndex(android.provider.MediaStore.Files.FileColumns.DISPLAY_NAME)
                if (nameIndex >= 0) {
                    it.moveToFirst()
                    return it.getString(nameIndex)
                }
            }
        } catch (e: Exception) {
            Log.e("ConnectedApp", "Error getting file name", e)
        }
        return null
    }

    fun deviceCount(): Int {
        return devices.size
    }

    fun cleanup() {
        uniffi.connected_ffi.shutdown()
    }
}
