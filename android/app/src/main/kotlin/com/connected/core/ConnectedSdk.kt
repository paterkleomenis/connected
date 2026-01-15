package com.connected.core

import android.content.Context
import android.net.ConnectivityManager
import android.net.LinkProperties
import android.net.NetworkCapabilities
import android.net.Uri
import android.net.wifi.WifiManager
import android.util.Log
import java.net.Inet4Address
import java.net.NetworkInterface
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.asSharedFlow
import java.io.File

data class DiscoveredDevice(
    val id: String,
    val name: String,
    val ip: String,
    val port: Int,
    val deviceType: String
)

sealed class DiscoveryEvent {
    data class DeviceFound(val device: DiscoveredDevice) : DiscoveryEvent()
    data class DeviceLost(val deviceId: String) : DiscoveryEvent()
    data class Error(val message: String) : DiscoveryEvent()
}

sealed class FileTransferEvent {
    data class Starting(val filename: String, val totalSize: Long) : FileTransferEvent()
    data class Progress(val bytesTransferred: Long, val totalSize: Long) : FileTransferEvent()
    data class Completed(val filename: String, val totalSize: Long) : FileTransferEvent()
    data class Failed(val error: String) : FileTransferEvent()
    object Cancelled : FileTransferEvent()
}

sealed class ClipboardEvent {
    data class Received(val text: String, val fromDevice: String) : ClipboardEvent()
    data class Sent(val success: Boolean, val error: String?) : ClipboardEvent()
}

sealed class PairingEvent {
    data class Request(val deviceName: String, val fingerprint: String, val deviceId: String) : PairingEvent()
}

class ConnectedSdk private constructor() {

    companion object {
        private const val TAG = "ConnectedSdk"
        private const val DEFAULT_PORT = 0 // Dynamic port
        private const val MULTICAST_LOCK_TAG = "ConnectedSdk_MulticastLock"

        @Volatile
        private var instance: ConnectedSdk? = null

        fun getInstance(): ConnectedSdk {
            return instance ?: synchronized(this) {
                instance ?: ConnectedSdk().also { instance = it }
            }
        }

        init {
            System.loadLibrary("connected_ffi")
        }
    }

    private val scope = CoroutineScope(Dispatchers.IO + SupervisorJob())

    private val _discoveryEvents = MutableSharedFlow<DiscoveryEvent>(replay = 0, extraBufferCapacity = 64)
    val discoveryEvents: SharedFlow<DiscoveryEvent> = _discoveryEvents.asSharedFlow()

    private val _fileTransferEvents = MutableSharedFlow<FileTransferEvent>(replay = 0, extraBufferCapacity = 64)
    val fileTransferEvents: SharedFlow<FileTransferEvent> = _fileTransferEvents.asSharedFlow()

    private val _clipboardEvents = MutableSharedFlow<ClipboardEvent>(replay = 0, extraBufferCapacity = 64)
    val clipboardEvents: SharedFlow<ClipboardEvent> = _clipboardEvents.asSharedFlow()

    private val _pairingEvents = MutableSharedFlow<PairingEvent>(replay = 0, extraBufferCapacity = 64)
    val pairingEvents: SharedFlow<PairingEvent> = _pairingEvents.asSharedFlow()

    private var isInitialized = false
    private var isDiscovering = false
    private var isClipboardSyncEnabled = false
    private var clipboardSyncJob: kotlinx.coroutines.Job? = null
    private var lastLocalClipboard: String = ""
    private var lastRemoteClipboard: String = ""
    private var clipboardSyncTargetIp: String? = null
    private var clipboardSyncTargetPort: Int = DEFAULT_PORT

    // WiFi Multicast Lock - required for mDNS on Android
    private var multicastLock: WifiManager.MulticastLock? = null
    private var appContext: Context? = null

    private val discoveryCallback = object : uniffi.connected_ffi.DiscoveryCallback {
        override fun onDeviceFound(device: uniffi.connected_ffi.DiscoveredDevice) {
            val mappedDevice = DiscoveredDevice(
                id = device.id,
                name = device.name,
                ip = device.ip,
                port = device.port.toInt(),
                deviceType = device.deviceType
            )
            scope.launch {
                _discoveryEvents.emit(DiscoveryEvent.DeviceFound(mappedDevice))
            }
            Log.d(TAG, "Device found: ${device.name} at ${device.ip}:${device.port}")
        }

        override fun onDeviceLost(deviceId: String) {
            scope.launch {
                _discoveryEvents.emit(DiscoveryEvent.DeviceLost(deviceId))
            }
            Log.d(TAG, "Device lost: $deviceId")
        }

        override fun onError(message: String) {
            scope.launch {
                _discoveryEvents.emit(DiscoveryEvent.Error(message))
            }
            Log.e(TAG, "Discovery error: $message")
        }
    }

    private val fileTransferCallback = object : uniffi.connected_ffi.FileTransferCallback {
        override fun onTransferRequest(transferId: String, filename: String, fileSize: ULong, fromDevice: String) {
            // This is handled by the global callback in ConnectedApp
            Log.d(TAG, "Transfer request from $fromDevice: $filename")
        }

        override fun onTransferStarting(filename: String, totalSize: ULong) {
            scope.launch {
                _fileTransferEvents.emit(FileTransferEvent.Starting(filename, totalSize.toLong()))
            }
            Log.d(TAG, "Transfer starting: $filename ($totalSize bytes)")
        }

        override fun onTransferProgress(bytesTransferred: ULong, totalSize: ULong) {
            scope.launch {
                _fileTransferEvents.emit(FileTransferEvent.Progress(bytesTransferred.toLong(), totalSize.toLong()))
            }
        }

        override fun onTransferCompleted(filename: String, totalSize: ULong) {
            scope.launch {
                _fileTransferEvents.emit(FileTransferEvent.Completed(filename, totalSize.toLong()))
            }
            Log.d(TAG, "Transfer completed: $filename")
        }

        override fun onTransferFailed(errorMsg: String) {
            scope.launch {
                _fileTransferEvents.emit(FileTransferEvent.Failed(errorMsg))
            }
            Log.e(TAG, "Transfer failed: $errorMsg")
        }

        override fun onTransferCancelled() {
            scope.launch {
                _fileTransferEvents.emit(FileTransferEvent.Cancelled)
            }
            Log.d(TAG, "Transfer cancelled")
        }
    }

    private val clipboardCallback = object : uniffi.connected_ffi.ClipboardCallback {
        override fun onClipboardReceived(text: String, fromDevice: String) {
            // For sync: update last remote clipboard to prevent echo
            if (isClipboardSyncEnabled && text != lastRemoteClipboard && text != lastLocalClipboard) {
                lastRemoteClipboard = text
                scope.launch(Dispatchers.Main) {
                    updateSyncedClipboard(appContext!!, text)
                    _clipboardEvents.emit(ClipboardEvent.Received(text, fromDevice))
                }
                Log.d(TAG, "Clipboard sync received: ${text.take(50)}...")
            } else if (!isClipboardSyncEnabled) {
                scope.launch(Dispatchers.Main) {
                    // Even if not syncing, we should copy to clipboard if explicitly sent
                    updateSyncedClipboard(appContext!!, text)
                    _clipboardEvents.emit(ClipboardEvent.Received(text, fromDevice))
                }
                Log.d(TAG, "Clipboard received from $fromDevice: ${text.take(50)}...")
            }
        }

        override fun onClipboardSent(success: Boolean, errorMsg: String?) {
            if (!isClipboardSyncEnabled) {
                scope.launch {
                    _clipboardEvents.emit(ClipboardEvent.Sent(success, errorMsg))
                }
            }
            if (success) {
                Log.d(TAG, "Clipboard sent successfully")
            } else {
                Log.e(TAG, "Clipboard send failed: $errorMsg")
            }
        }
    }

    private val pairingCallback = object : uniffi.connected_ffi.PairingCallback {
        override fun onPairingRequest(deviceName: String, fingerprint: String, deviceId: String) {
            scope.launch {
                _pairingEvents.emit(PairingEvent.Request(deviceName, fingerprint, deviceId))
            }
            Log.d(TAG, "Pairing request from $deviceName ($fingerprint)")
        }
    }

    @Throws(ConnectedException::class)
    fun initialize(context: Context, deviceName: String, port: Int = DEFAULT_PORT) {
        if (isInitialized) {
            Log.w(TAG, "SDK already initialized")
            return
        }

        // Store application context for multicast lock
        appContext = context.applicationContext

        // Acquire multicast lock BEFORE initialization so mDNS announcements work
        acquireMulticastLock()

        // Use external files dir for consistency with ConnectedApp
        // This ensures the same identity and known_peers are used
        val storagePath = context.getExternalFilesDir(null)?.absolutePath ?: context.filesDir.absolutePath

        try {
            // First try the default initialization
            try {
                uniffi.connected_ffi.initialize(
                    deviceName = deviceName,
                    deviceType = "android",
                    bindPort = port.toUShort(),
                    storagePath = storagePath
                )
            } catch (e: uniffi.connected_ffi.ConnectedFfiException) {
                // If default init fails, try with explicit IP from Android APIs
                Log.w(TAG, "Default init failed, trying with explicit IP: ${e.message}")
                val localIp = getLocalIpAddress(context)
                if (localIp != null) {
                    Log.i(TAG, "Using detected IP: $localIp")
                    uniffi.connected_ffi.initializeWithIp(
                        deviceName = deviceName,
                        deviceType = "android",
                        bindPort = port.toUShort(),
                        ipAddress = localIp,
                        storagePath = storagePath
                    )
                } else {
                    throw e
                }
            }

            // Register Global Callbacks
            uniffi.connected_ffi.registerTransferCallback(fileTransferCallback)
            uniffi.connected_ffi.registerClipboardReceiver(clipboardCallback)
            uniffi.connected_ffi.registerPairingCallback(pairingCallback)

            isInitialized = true
            Log.i(TAG, "Connected SDK initialized successfully")
        } catch (e: uniffi.connected_ffi.ConnectedFfiException) {
            releaseMulticastLock()
            throw ConnectedException("Failed to initialize: ${e.message}", e)
        }
    }

    private fun getLocalIpAddress(context: Context): String? {
        // Method 1: Try ConnectivityManager (modern approach)
        try {
            val connectivityManager = context.getSystemService(Context.CONNECTIVITY_SERVICE) as? ConnectivityManager
            if (connectivityManager != null) {
                val activeNetwork = connectivityManager.activeNetwork
                val linkProperties = connectivityManager.getLinkProperties(activeNetwork)
                linkProperties?.linkAddresses?.forEach { linkAddress ->
                    val address = linkAddress.address
                    if (address is Inet4Address && !address.isLoopbackAddress) {
                        val ip = address.hostAddress
                        if (ip != null && (ip.startsWith("192.168") || ip.startsWith("10.") || ip.startsWith("172."))) {
                            Log.d(TAG, "Found IP via ConnectivityManager: $ip")
                            return ip
                        }
                    }
                }
            }
        } catch (e: Exception) {
            Log.w(TAG, "ConnectivityManager method failed: ${e.message}")
        }

        // Method 2: Try WifiManager (older approach) - REMOVED due to deprecation

        // We rely on ConnectivityManager (Method 1) and NetworkInterface (Method 3)


        // Method 3: Enumerate network interfaces
        try {
            NetworkInterface.getNetworkInterfaces()?.toList()?.forEach { networkInterface ->
                if (networkInterface.isUp && !networkInterface.isLoopback) {
                    networkInterface.inetAddresses?.toList()?.forEach { address ->
                        if (address is Inet4Address && !address.isLoopbackAddress) {
                            val ip = address.hostAddress
                            if (ip != null && (ip.startsWith("192.168") || ip.startsWith("10.") || ip.startsWith("172."))) {
                                Log.d(TAG, "Found IP via NetworkInterface: $ip")
                                return ip
                            }
                        }
                    }
                }
            }
        } catch (e: Exception) {
            Log.w(TAG, "NetworkInterface method failed: ${e.message}")
        }

        Log.e(TAG, "Could not determine local IP address")
        return null
    }

    @Throws(ConnectedException::class)
    fun startDiscovery() {
        checkInitialized()

        if (isDiscovering) {
            Log.w(TAG, "Discovery already running")
            return
        }

        try {
            // Clear any stale devices from previous sessions before starting fresh
            try {
                uniffi.connected_ffi.clearDiscoveredDevices()
                Log.d(TAG, "Cleared previous discovered devices")
            } catch (e: Exception) {
                Log.w(TAG, "Failed to clear discovered devices: ${e.message}")
            }

            // Multicast lock is already acquired during initialize()
            uniffi.connected_ffi.startDiscovery(discoveryCallback)
            isDiscovering = true
            Log.i(TAG, "Discovery started")
        } catch (e: uniffi.connected_ffi.ConnectedFfiException) {
            throw ConnectedException("Failed to start discovery: ${e.message}", e)
        }
    }

    fun clearDiscoveredDevices() {
        try {
            uniffi.connected_ffi.clearDiscoveredDevices()
            Log.d(TAG, "Discovered devices cleared")
        } catch (e: Exception) {
            Log.w(TAG, "Failed to clear discovered devices: ${e.message}")
        }
    }

    @Throws(ConnectedException::class)
    fun stopDiscovery() {
        checkInitialized()

        if (!isDiscovering) {
            return
        }

        try {
            uniffi.connected_ffi.stopDiscovery()
            isDiscovering = false
            // Don't release multicast lock - keep it for mDNS announcements
            Log.i(TAG, "Discovery stopped")
        } catch (e: uniffi.connected_ffi.ConnectedFfiException) {
            throw ConnectedException("Failed to stop discovery: ${e.message}", e)
        }
    }

    @Throws(ConnectedException::class)
    fun getDiscoveredDevices(): List<DiscoveredDevice> {
        checkInitialized()

        return try {
            uniffi.connected_ffi.getDiscoveredDevices().map { device ->
                DiscoveredDevice(
                    id = device.id,
                    name = device.name,
                    ip = device.ip,
                    port = device.port.toInt(),
                    deviceType = device.deviceType
                )
            }
        } catch (e: uniffi.connected_ffi.ConnectedFfiException) {
            throw ConnectedException("Failed to get devices: ${e.message}", e)
        }
    }

    @Throws(ConnectedException::class)
    fun getLocalDevice(): DiscoveredDevice {
        checkInitialized()

        return try {
            val device = uniffi.connected_ffi.getLocalDevice()
            DiscoveredDevice(
                id = device.id,
                name = device.name,
                ip = device.ip,
                port = device.port.toInt(),
                deviceType = device.deviceType
            )
        } catch (e: uniffi.connected_ffi.ConnectedFfiException) {
            throw ConnectedException("Failed to get local device: ${e.message}", e)
        }
    }

    @Throws(ConnectedException::class)
    fun getLocalFingerprint(): String {
        checkInitialized()
        return try {
            uniffi.connected_ffi.getLocalFingerprint()
        } catch (e: uniffi.connected_ffi.ConnectedFfiException) {
            throw ConnectedException("Failed to get local fingerprint: ${e.message}", e)
        }
    }

    fun shutdown() {
        if (!isInitialized) {
            return
        }

        try {
            if (isDiscovering) {
                stopDiscovery()
            }
            releaseMulticastLock()
            uniffi.connected_ffi.shutdown()
            isInitialized = false
            isDiscovering = false
            scope.cancel()
            Log.i(TAG, "Connected SDK shut down")
        } catch (e: Exception) {
            Log.e(TAG, "Error during shutdown: ${e.message}")
        }
    }

    private fun checkInitialized() {
        if (!isInitialized) {
            throw ConnectedException("SDK not initialized. Call initialize() first.")
        }
    }

    @Throws(ConnectedException::class)
    fun sendFile(targetIp: String, targetPort: Int = DEFAULT_PORT, filePath: String) {
        checkInitialized()

        val file = File(filePath)
        if (!file.exists()) {
            throw ConnectedException("File not found: $filePath")
        }

        try {
            // Updated: No callback needed here, global callback handles it
            uniffi.connected_ffi.sendFile(
                targetIp = targetIp,
                targetPort = targetPort.toUShort(),
                filePath = filePath
            )
            Log.i(TAG, "File transfer started: $filePath -> $targetIp:$targetPort")
        } catch (e: uniffi.connected_ffi.ConnectedFfiException) {
            throw ConnectedException("Failed to send file: ${e.message}", e)
        }
    }

    // Removed startFileReceiver (handled globally/automatically)

    @Throws(ConnectedException::class)
    fun sendClipboard(targetIp: String, targetPort: Int, text: String) {
        checkInitialized()

        if (text.isEmpty()) {
            throw ConnectedException("Clipboard text is empty")
        }

        try {
            uniffi.connected_ffi.sendClipboard(
                targetIp = targetIp,
                targetPort = targetPort.toUShort(),
                text = text,
                callback = clipboardCallback
            )
            Log.i(TAG, "Clipboard send started to $targetIp:$targetPort (${text.length} chars)")
        } catch (e: uniffi.connected_ffi.ConnectedFfiException) {
            throw ConnectedException("Failed to send clipboard: ${e.message}", e)
        }
    }

    fun startClipboardSync(
        context: Context,
        targetIp: String,
        targetPort: Int,
        intervalMs: Long = 500
    ) {
        checkInitialized()

        if (isClipboardSyncEnabled) {
            Log.w(TAG, "Clipboard sync already running")
            return
        }

        clipboardSyncTargetIp = targetIp
        clipboardSyncTargetPort = targetPort
        isClipboardSyncEnabled = true

        val clipboardManager = context.getSystemService(Context.CLIPBOARD_SERVICE) as android.content.ClipboardManager

        // Initialize with current clipboard
        clipboardManager.primaryClip?.getItemAt(0)?.text?.toString()?.let {
            lastLocalClipboard = it
        }

        clipboardSyncJob = scope.launch {
            Log.i(TAG, "Clipboard sync started with $targetIp:$targetPort")

            while (isActive && isClipboardSyncEnabled) {
                try {
                    // Check for local clipboard changes
                    val currentClipboard = withContext(Dispatchers.Main) {
                        clipboardManager.primaryClip?.getItemAt(0)?.text?.toString() ?: ""
                    }

                    if (currentClipboard.isNotEmpty() &&
                        currentClipboard != lastLocalClipboard &&
                        currentClipboard != lastRemoteClipboard
                    ) {
                        Log.d(TAG, "Clipboard changed, syncing: ${currentClipboard.take(30)}...")
                        lastLocalClipboard = currentClipboard

                        // Send to remote
                        try {
                            uniffi.connected_ffi.sendClipboard(
                                targetIp = targetIp,
                                targetPort = targetPort.toUShort(),
                                text = currentClipboard,
                                callback = clipboardCallback
                            )
                        } catch (e: Exception) {
                            Log.e(TAG, "Failed to sync clipboard: ${e.message}")
                        }
                    }
                } catch (e: Exception) {
                    Log.e(TAG, "Clipboard sync error: ${e.message}")
                }

                delay(intervalMs)
            }
        }
    }

    fun stopClipboardSync() {
        if (!isClipboardSyncEnabled) {
            return
        }

        isClipboardSyncEnabled = false
        clipboardSyncJob?.cancel()
        clipboardSyncJob = null
        clipboardSyncTargetIp = null
        lastLocalClipboard = ""
        lastRemoteClipboard = ""
        Log.i(TAG, "Clipboard sync stopped")
    }

    fun isClipboardSyncRunning(): Boolean = isClipboardSyncEnabled

    fun updateSyncedClipboard(context: Context, text: String) {
        if (text == lastRemoteClipboard) return

        lastRemoteClipboard = text
        val clipboardManager = context.getSystemService(Context.CLIPBOARD_SERVICE) as android.content.ClipboardManager
        clipboardManager.setPrimaryClip(android.content.ClipData.newPlainText("Connected", text))
        Log.d(TAG, "Clipboard updated from sync: ${text.take(30)}...")
    }

    // Pairing API
    fun setPairingMode(enabled: Boolean) {
        checkInitialized()
        try {
            uniffi.connected_ffi.setPairingMode(enabled)
            Log.d(TAG, "Pairing mode set to $enabled")
        } catch (e: Exception) {
            Log.e(TAG, "Failed to set pairing mode: ${e.message}")
        }
    }

    fun trustDevice(fingerprint: String, deviceId: String, name: String) {
        checkInitialized()
        try {
            uniffi.connected_ffi.trustDevice(fingerprint, deviceId, name)
            Log.d(TAG, "Trusted device $name ($fingerprint, $deviceId)")
        } catch (e: Exception) {
            Log.e(TAG, "Failed to trust device: ${e.message}")
        }
    }

    private fun acquireMulticastLock() {
        if (multicastLock != null && multicastLock!!.isHeld) {
            Log.d(TAG, "Multicast lock already held")
            return
        }

        val context = appContext ?: run {
            Log.w(TAG, "No context available for multicast lock")
            return
        }

        try {
            val wifiManager = context.getSystemService(Context.WIFI_SERVICE) as? WifiManager
            if (wifiManager != null) {
                multicastLock = wifiManager.createMulticastLock(MULTICAST_LOCK_TAG).apply {
                    setReferenceCounted(true)
                    acquire()
                }
                Log.i(TAG, "Multicast lock acquired")
            } else {
                Log.w(TAG, "WifiManager not available")
            }
        } catch (e: Exception) {
            Log.e(TAG, "Failed to acquire multicast lock: ${e.message}")
        }
    }

    private fun releaseMulticastLock() {
        try {
            multicastLock?.let { lock ->
                if (lock.isHeld) {
                    lock.release()
                    Log.i(TAG, "Multicast lock released")
                }
            }
            multicastLock = null
        } catch (e: Exception) {
            Log.e(TAG, "Failed to release multicast lock: ${e.message}")
        }
    }
}

class ConnectedException(message: String, cause: Throwable? = null) : Exception(message, cause)
