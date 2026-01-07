package com.connected.app

import android.content.Context
import android.util.Log
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import uniffi.connected_ffi.*

class ConnectedApp(private val context: Context) {
    // State exposed to Compose
    val devices = mutableStateListOf<DiscoveredDevice>()
    val trustedDevices = mutableStateListOf<String>() // Set of trusted Device IDs
    val pendingPairing = mutableStateListOf<String>() // Set of pending Device IDs
    val transferStatus = mutableStateOf("Idle")
    val clipboardContent = mutableStateOf("")
    val pairingRequest = mutableStateOf<PairingRequest?>(null)

    data class PairingRequest(val deviceName: String, val fingerprint: String, val deviceId: String)

    private val discoveryCallback = object : DiscoveryCallback {
        override fun onDeviceFound(device: DiscoveredDevice) {
            Log.d("ConnectedApp", "Device found: ${device.name}")
            if (devices.none { it.id == device.id }) {
                devices.add(device)
            }
            // Check trust status for new device
            if (isDeviceTrusted(device) && !trustedDevices.contains(device.id)) {
                trustedDevices.add(device.id)
                // If it was pending, remove it
                if (pendingPairing.contains(device.id)) {
                    pendingPairing.remove(device.id)
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
        override fun onTransferStarting(filename: String, totalSize: ULong) {
            transferStatus.value = "Starting transfer: $filename"
        }

        override fun onTransferProgress(bytesTransferred: ULong, totalSize: ULong) {
            val percent = if (totalSize > 0u) (bytesTransferred.toLong() * 100 / totalSize.toLong()) else 0
            transferStatus.value = "Transferring: $percent%"
        }

        override fun onTransferCompleted(filename: String, totalSize: ULong) {
            transferStatus.value = "Completed: $filename"
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

    fun initialize() {
        try {
            uniffi.connected_ffi.initialize(
                "Android Device",
                "Mobile",
                0u.toUShort(),
                context.filesDir.absolutePath
            )
            uniffi.connected_ffi.startDiscovery(discoveryCallback)
            uniffi.connected_ffi.registerTransferCallback(transferCallback)
            uniffi.connected_ffi.registerClipboardReceiver(clipboardCallback)
            uniffi.connected_ffi.registerPairingCallback(pairingCallback)
        } catch (e: Exception) {
            Log.e("ConnectedApp", "Initialization failed", e)
        }
    }

    fun startDiscovery() {
        // Already started in initialize, but exposed if needed to restart
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
        try {
            uniffi.connected_ffi.unpairDevice(device.id)
            trustedDevices.remove(device.id)
            if (pendingPairing.contains(device.id)) {
                pendingPairing.remove(device.id)
            }
            getDevices()
            android.widget.Toast.makeText(context, "Unpaired ${device.name}", android.widget.Toast.LENGTH_SHORT).show()
        } catch (e: Exception) {
            Log.e("ConnectedApp", "Unpair failed", e)
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
            uniffi.connected_ffi.trustDevice(request.fingerprint, request.deviceName)
            pairingRequest.value = null

            // Send handshake back to confirm pairing
            val device = devices.find { it.id == request.deviceId }
            if (device != null) {
                pairDevice(device)
                if (!trustedDevices.contains(device.id)) {
                    trustedDevices.add(device.id)
                }
            }
            getDevices()
        } catch (e: Exception) {
            Log.e("ConnectedApp", "Trust device failed", e)
        }
    }

    fun rejectDevice(request: PairingRequest) {
        // We can block it or just ignore. For now, just clear request.
        pairingRequest.value = null
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

    fun deviceCount(): Int {
        return devices.size
    }

    fun cleanup() {
        uniffi.connected_ffi.shutdown()
    }
}
