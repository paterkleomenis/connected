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

    // Track when other devices unpair us
    val unpairNotification = mutableStateOf<String?>(null)

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
            uniffi.connected_ffi.registerUnpairCallback(unpairCallback)
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
