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

data class PingResult(
    val success: Boolean,
    val rttMs: Long,
    val errorMessage: String?
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

class ConnectedSdk private constructor() {

    companion object {
        private const val TAG = "ConnectedSdk"
        private const val DEFAULT_PORT = 44444
        private const val MULTICAST_LOCK_TAG = "ConnectedSdk_MulticastLock"

        @Volatile
        private var instance: ConnectedSdk? = null

        fun getInstance(): ConnectedSdk {
            return instance ?: synchronized(this) {
                instance ?: ConnectedSdk().also { instance = it }
            }
        }

        init {
            System.loadLibrary("connected_core")
        }
    }

    private val scope = CoroutineScope(Dispatchers.IO + SupervisorJob())

    private val _discoveryEvents = MutableSharedFlow<DiscoveryEvent>(replay = 0, extraBufferCapacity = 64)
    val discoveryEvents: SharedFlow<DiscoveryEvent> = _discoveryEvents.asSharedFlow()

    private val _fileTransferEvents = MutableSharedFlow<FileTransferEvent>(replay = 0, extraBufferCapacity = 64)
    val fileTransferEvents: SharedFlow<FileTransferEvent> = _fileTransferEvents.asSharedFlow()

    private val _clipboardEvents = MutableSharedFlow<ClipboardEvent>(replay = 0, extraBufferCapacity = 64)
    val clipboardEvents: SharedFlow<ClipboardEvent> = _clipboardEvents.asSharedFlow()

    private var isInitialized = false
    private var isDiscovering = false
    private var isReceivingFiles = false
    private var isClipboardSyncEnabled = false
    private var clipboardSyncJob: kotlinx.coroutines.Job? = null
    private var lastLocalClipboard: String = ""
    private var lastRemoteClipboard: String = ""
    private var clipboardSyncTargetIp: String? = null
    private var clipboardSyncTargetPort: Int = DEFAULT_PORT + 1

    // WiFi Multicast Lock - required for mDNS on Android
    private var multicastLock: WifiManager.MulticastLock? = null
    private var appContext: Context? = null

    private val discoveryCallback = object : uniffi.connected_core.DiscoveryCallback {
        override fun onDeviceFound(device: uniffi.connected_core.DiscoveredDevice) {
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

    private val fileTransferCallback = object : uniffi.connected_core.FileTransferCallback {
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

    private val clipboardCallback = object : uniffi.connected_core.ClipboardCallback {
        override fun onClipboardReceived(text: String, fromDevice: String) {
            // For sync: update last remote clipboard to prevent echo
            if (isClipboardSyncEnabled && text != lastRemoteClipboard && text != lastLocalClipboard) {
                lastRemoteClipboard = text
                scope.launch {
                    _clipboardEvents.emit(ClipboardEvent.Received(text, fromDevice))
                }
                Log.d(TAG, "Clipboard sync received: ${text.take(50)}...")
            } else if (!isClipboardSyncEnabled) {
                scope.launch {
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

        try {
            // First try the default initialization
            try {
                uniffi.connected_core.initialize(
                    deviceName = deviceName,
                    deviceType = "android",
                    bindPort = port.toUShort()
                )
            } catch (e: uniffi.connected_core.ConnectedFfiException) {
                // If default init fails, try with explicit IP from Android APIs
                Log.w(TAG, "Default init failed, trying with explicit IP: ${e.message}")
                val localIp = getLocalIpAddress(context)
                if (localIp != null) {
                    Log.i(TAG, "Using detected IP: $localIp")
                    uniffi.connected_core.initializeWithIp(
                        deviceName = deviceName,
                        deviceType = "android",
                        bindPort = port.toUShort(),
                        ipAddress = localIp
                    )
                } else {
                    throw e
                }
            }
            isInitialized = true
            Log.i(TAG, "Connected SDK initialized successfully")
        } catch (e: uniffi.connected_core.ConnectedFfiException) {
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

        // Method 2: Try WifiManager (older approach)
        try {
            val wifiManager = context.applicationContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
            if (wifiManager != null) {
                val wifiInfo = wifiManager.connectionInfo
                val ipInt = wifiInfo.ipAddress
                if (ipInt != 0) {
                    val ip = String.format(
                        "%d.%d.%d.%d",
                        ipInt and 0xff,
                        ipInt shr 8 and 0xff,
                        ipInt shr 16 and 0xff,
                        ipInt shr 24 and 0xff
                    )
                    Log.d(TAG, "Found IP via WifiManager: $ip")
                    return ip
                }
            }
        } catch (e: Exception) {
            Log.w(TAG, "WifiManager method failed: ${e.message}")
        }

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
                uniffi.connected_core.clearDiscoveredDevices()
                Log.d(TAG, "Cleared previous discovered devices")
            } catch (e: Exception) {
                Log.w(TAG, "Failed to clear discovered devices: ${e.message}")
            }

            // Multicast lock is already acquired during initialize()
            uniffi.connected_core.startDiscovery(discoveryCallback)
            isDiscovering = true
            Log.i(TAG, "Discovery started")
        } catch (e: uniffi.connected_core.ConnectedFfiException) {
            throw ConnectedException("Failed to start discovery: ${e.message}", e)
        }
    }

    fun clearDiscoveredDevices() {
        try {
            uniffi.connected_core.clearDiscoveredDevices()
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
            uniffi.connected_core.stopDiscovery()
            isDiscovering = false
            // Don't release multicast lock - keep it for mDNS announcements
            Log.i(TAG, "Discovery stopped")
        } catch (e: uniffi.connected_core.ConnectedFfiException) {
            throw ConnectedException("Failed to stop discovery: ${e.message}", e)
        }
    }

    @Throws(ConnectedException::class)
    fun getDiscoveredDevices(): List<DiscoveredDevice> {
        checkInitialized()

        return try {
            uniffi.connected_core.getDiscoveredDevices().map { device ->
                DiscoveredDevice(
                    id = device.id,
                    name = device.name,
                    ip = device.ip,
                    port = device.port.toInt(),
                    deviceType = device.deviceType
                )
            }
        } catch (e: uniffi.connected_core.ConnectedFfiException) {
            throw ConnectedException("Failed to get devices: ${e.message}", e)
        }
    }

    @Throws(ConnectedException::class)
    suspend fun ping(targetIp: String, targetPort: Int = DEFAULT_PORT): PingResult {
        checkInitialized()

        return withContext(Dispatchers.IO) {
            try {
                val result = uniffi.connected_core.sendPing(targetIp, targetPort.toUShort())
                PingResult(
                    success = result.success,
                    rttMs = result.rttMs.toLong(),
                    errorMessage = result.errorMessage
                )
            } catch (e: uniffi.connected_core.ConnectedFfiException) {
                PingResult(
                    success = false,
                    rttMs = 0,
                    errorMessage = e.message
                )
            }
        }
    }

    @Throws(ConnectedException::class)
    fun getLocalDevice(): DiscoveredDevice {
        checkInitialized()

        return try {
            val device = uniffi.connected_core.getLocalDevice()
            DiscoveredDevice(
                id = device.id,
                name = device.name,
                ip = device.ip,
                port = device.port.toInt(),
                deviceType = device.deviceType
            )
        } catch (e: uniffi.connected_core.ConnectedFfiException) {
            throw ConnectedException("Failed to get local device: ${e.message}", e)
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
            uniffi.connected_core.shutdown()
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
            uniffi.connected_core.sendFile(
                targetIp = targetIp,
                targetPort = targetPort.toUShort(),
                filePath = filePath,
                callback = fileTransferCallback
            )
            Log.i(TAG, "File transfer started: $filePath -> $targetIp:$targetPort")
        } catch (e: uniffi.connected_core.ConnectedFfiException) {
            throw ConnectedException("Failed to send file: ${e.message}", e)
        }
    }

    @Throws(ConnectedException::class)
    fun startFileReceiver(saveDir: String) {
        checkInitialized()

        if (isReceivingFiles) {
            Log.w(TAG, "File receiver already running")
            return
        }

        try {
            // Ensure directory exists
            val dir = File(saveDir)
            if (!dir.exists()) {
                dir.mkdirs()
            }

            uniffi.connected_core.startFileReceiver(
                saveDir = saveDir,
                callback = fileTransferCallback
            )
            isReceivingFiles = true
            Log.i(TAG, "File receiver started, saving to: $saveDir")
        } catch (e: uniffi.connected_core.ConnectedFfiException) {
            throw ConnectedException("Failed to start file receiver: ${e.message}", e)
        }
    }

    @Throws(ConnectedException::class)
    fun sendClipboard(targetIp: String, targetPort: Int = DEFAULT_PORT + 1, text: String) {
        checkInitialized()

        if (text.isEmpty()) {
            throw ConnectedException("Clipboard text is empty")
        }

        try {
            uniffi.connected_core.sendClipboard(
                targetIp = targetIp,
                targetPort = targetPort.toUShort(),
                text = text,
                callback = clipboardCallback
            )
            Log.i(TAG, "Clipboard send started to $targetIp:$targetPort (${text.length} chars)")
        } catch (e: uniffi.connected_core.ConnectedFfiException) {
            throw ConnectedException("Failed to send clipboard: ${e.message}", e)
        }
    }

    fun registerClipboardReceiver() {
        checkInitialized()

        try {
            uniffi.connected_core.registerClipboardReceiver(clipboardCallback)
            Log.i(TAG, "Clipboard receiver registered")
        } catch (e: Exception) {
            Log.e(TAG, "Failed to register clipboard receiver: ${e.message}")
        }
    }

    fun startClipboardSync(
        context: Context,
        targetIp: String,
        targetPort: Int = DEFAULT_PORT + 1,
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
                            uniffi.connected_core.sendClipboard(
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
