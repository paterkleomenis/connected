package com.connected.app

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.graphics.BitmapFactory
import android.net.Uri
import android.os.Build
import android.provider.OpenableColumns
import android.provider.Settings
import android.support.v4.media.MediaMetadataCompat
import android.support.v4.media.session.MediaSessionCompat
import android.support.v4.media.session.PlaybackStateCompat
import android.util.Log
import androidx.annotation.RequiresApi
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.core.content.ContextCompat
import androidx.core.content.edit
import androidx.core.net.toUri
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import uniffi.connected_ffi.CallAction
import uniffi.connected_ffi.ClipboardCallback
import uniffi.connected_ffi.DiscoveredDevice
import uniffi.connected_ffi.DiscoveryCallback
import uniffi.connected_ffi.FfiActiveCall
import uniffi.connected_ffi.FfiCallLogEntry
import uniffi.connected_ffi.FfiContact
import uniffi.connected_ffi.FfiConversation
import uniffi.connected_ffi.FfiFsEntry
import uniffi.connected_ffi.FfiSmsMessage
import uniffi.connected_ffi.FileTransferCallback
import uniffi.connected_ffi.MediaCommand
import uniffi.connected_ffi.MediaControlCallback
import uniffi.connected_ffi.MediaState
import uniffi.connected_ffi.PairingCallback
import uniffi.connected_ffi.SmsStatus
import uniffi.connected_ffi.TelephonyCallback
import uniffi.connected_ffi.UnpairCallback
import uniffi.connected_ffi.UpdateInfo
import uniffi.connected_ffi.acceptFileTransfer
import uniffi.connected_ffi.checkForUpdates
import uniffi.connected_ffi.forgetDeviceById
import uniffi.connected_ffi.getDiscoveredDevices
import uniffi.connected_ffi.initialize
import uniffi.connected_ffi.isDeviceTrusted
import uniffi.connected_ffi.notifyNewSms
import uniffi.connected_ffi.pairDevice
import uniffi.connected_ffi.registerClipboardReceiver
import uniffi.connected_ffi.registerFilesystemProvider
import uniffi.connected_ffi.registerMediaControlCallback
import uniffi.connected_ffi.registerPairingCallback
import uniffi.connected_ffi.registerTelephonyCallback
import uniffi.connected_ffi.registerTransferCallback
import uniffi.connected_ffi.registerUnpairCallback
import uniffi.connected_ffi.rejectFileTransfer
import uniffi.connected_ffi.rejectPairing
import uniffi.connected_ffi.requestDownloadFile
import uniffi.connected_ffi.requestDownloadFileWithProgress
import uniffi.connected_ffi.requestDownloadFolder
import uniffi.connected_ffi.requestGetThumbnail
import uniffi.connected_ffi.requestListDir
import uniffi.connected_ffi.BrowserDownloadCallback
import uniffi.connected_ffi.sendActiveCallUpdate
import uniffi.connected_ffi.sendCallLog
import uniffi.connected_ffi.sendClipboard
import uniffi.connected_ffi.sendContacts
import uniffi.connected_ffi.sendConversations
import uniffi.connected_ffi.sendFile
import uniffi.connected_ffi.sendMediaCommand
import uniffi.connected_ffi.sendMediaState
import uniffi.connected_ffi.sendMessages
import uniffi.connected_ffi.sendSmsSendResult
import uniffi.connected_ffi.sendTrustConfirmation
import uniffi.connected_ffi.setPairingMode
import uniffi.connected_ffi.shutdown
import uniffi.connected_ffi.refreshDiscovery
import uniffi.connected_ffi.startDiscovery
import uniffi.connected_ffi.trustDevice
import uniffi.connected_ffi.unpairDeviceById
import java.io.File
import java.io.FileOutputStream
import java.util.Collections
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.ConcurrentLinkedQueue
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference
import java.util.zip.ZipEntry
import java.util.zip.ZipOutputStream

class ConnectedApp(private val context: Context) {

    private fun hasProximityPermissions(): Boolean {
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.TIRAMISU) {
            if (ContextCompat.checkSelfPermission(
                    context,
                    Manifest.permission.NEARBY_WIFI_DEVICES
                ) != PackageManager.PERMISSION_GRANTED
            ) return false
        }
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.S) {
            if (ContextCompat.checkSelfPermission(
                    context,
                    Manifest.permission.BLUETOOTH_SCAN
                ) != PackageManager.PERMISSION_GRANTED
            ) return false
            if (ContextCompat.checkSelfPermission(
                    context,
                    Manifest.permission.BLUETOOTH_ADVERTISE
                ) != PackageManager.PERMISSION_GRANTED
            ) return false
            if (ContextCompat.checkSelfPermission(
                    context,
                    Manifest.permission.BLUETOOTH_CONNECT
                ) != PackageManager.PERMISSION_GRANTED
            ) return false
        } else {
            if (ContextCompat.checkSelfPermission(
                    context,
                    Manifest.permission.ACCESS_FINE_LOCATION
                ) != PackageManager.PERMISSION_GRANTED
            ) return false
        }
        return true
    }

    companion object {
        private const val NOTIFICATION_ID_REQUEST = 1001
        private const val NOTIFICATION_ID_PROGRESS = 1002
        private const val NOTIFICATION_ID_COMPLETE = 1003
        private const val MEDIA_NOTIFICATION_ID = 1004

        @Volatile
        @android.annotation.SuppressLint("StaticFieldLeak")
        private var instance: ConnectedApp? = null

        fun getInstance(context: Context): ConnectedApp {
            return instance ?: synchronized(this) {
                instance ?: ConnectedApp(context.applicationContext).also { instance = it }
            }
        }

        fun getInstance(): ConnectedApp? = instance
    }

    // State exposed to Compose
    val devices = mutableStateListOf<DiscoveredDevice>()
    val trustedDevices = mutableStateListOf<String>() // Set of trusted Device IDs
    val pendingPairing = mutableStateListOf<String>() // Set of pending Device IDs
    val pendingShareUris = mutableStateListOf<String>()

    private val pendingPairingAwaitingIp = mutableSetOf<String>()
    private val locallyUnpairedDevices = mutableSetOf<String>()

    // Stores pending file transfers with timestamp for cleanup: deviceId -> (timestamp, queue of file paths)
    private val pendingFileTransfersAwaitingIp =
        ConcurrentHashMap<String, Pair<Long, ConcurrentLinkedQueue<String>>>()
    val transferStatus = mutableStateOf("Idle")
    val compressionProgress = mutableStateOf<CompressionProgress?>(null)
    val browserDownloadProgress = mutableStateOf<BrowserDownloadProgress?>(null)

    data class BrowserDownloadProgress(
        val currentFile: String,
        val bytesDownloaded: Long,
        val totalBytes: Long,
        val isFolder: Boolean = false
    ) {
        val percentComplete: Float
            get() = if (totalBytes > 0) (bytesDownloaded.toFloat() / totalBytes * 100f) else 0f
    }

    data class CompressionProgress(
        val currentFile: String,
        val filesProcessed: Int,
        val totalFiles: Int,
        val bytesProcessed: Long,
        val totalBytes: Long,
        val speedBytesPerSec: Long
    ) {
        val percentComplete: Float
            get() = if (totalBytes > 0) (bytesProcessed.toFloat() / totalBytes * 100f) else 0f

        val estimatedSecondsRemaining: Long
            get() = if (speedBytesPerSec > 0) ((totalBytes - bytesProcessed) / speedBytesPerSec) else 0
    }

    val clipboardContent = mutableStateOf("")
    val pairingRequest = mutableStateOf<PairingRequest?>(null)
    val transferRequest = mutableStateOf<TransferRequest?>(null)

    // Telephony State
    val telephonyProvider = TelephonyProvider(context)
    val contacts = mutableStateListOf<FfiContact>()
    val conversations = mutableStateListOf<FfiConversation>()
    val currentMessages = mutableStateListOf<FfiSmsMessage>()
    val callLog = mutableStateListOf<FfiCallLogEntry>()
    val activeCall = mutableStateOf<FfiActiveCall?>(null)
    val isTelephonyEnabled = mutableStateOf(false)

    // Media Session
    private var mediaSession: MediaSessionCompat? = null
    val currentMediaTitle = mutableStateOf("Not Playing")
    val currentMediaArtist = mutableStateOf("")
    val currentMediaPlaying = mutableStateOf(false)
    private var lastMediaSourceDevice: String? = null
    private var lastBroadcastMediaState: MediaState? = null

    data class PairingRequest(val deviceName: String, val fingerprint: String, val deviceId: String)
    data class TransferRequest(val id: String, val filename: String, val fileSize: ULong, val fromDevice: String)

    // Clipboard Sync State
    val isClipboardSyncEnabled = mutableStateOf(false)
    val isMediaControlEnabled = mutableStateOf(false)
    private var clipboardSyncJob: kotlinx.coroutines.Job? = null

    private val lastRemoteClipboard = AtomicReference("")

    private val isAppInForeground = AtomicBoolean(false)
    private var scopeJob = kotlinx.coroutines.SupervisorJob()
    private var scope =
        kotlinx.coroutines.CoroutineScope(Dispatchers.IO + scopeJob)

    val unpairNotification = mutableStateOf<String?>(null)

    private var selectedDeviceForFile: DiscoveredDevice? = null
    private lateinit var downloadDir: File
    private var pendingUpdateInstall: File? = null

    // FilesystemProvider State
    val isFsProviderRegistered = mutableStateOf(false)
    val sharedFolderName = mutableStateOf<String?>(null)
    val remoteFiles = mutableStateListOf<FfiFsEntry>()
    val currentRemotePath = mutableStateOf("/")
    val isBrowsingRemote = mutableStateOf(false)
    private var browsingDevice: DiscoveredDevice? = null
    val thumbnails = androidx.compose.runtime.mutableStateMapOf<String, android.graphics.Bitmap>()
    private val requestedThumbnails = Collections.synchronizedSet(mutableSetOf<String>())
    private val thumbnailAccessOrder = mutableListOf<String>()
    private val thumbnailLock = Any() // Single lock for all thumbnail cache operations
    private val maxThumbnailCacheSize = 100 // Maximum number of cached thumbnails

    // Network State
    private var multicastLock: android.net.wifi.WifiManager.MulticastLock? = null
    private var proximityManager: ProximityManager? = null

    private val _prefsName = "ConnectedPrefs"
    private val _prefRootUri = "root_uri"
    private val _prefMediaControl = "media_control"
    private val _prefTelephonyEnabled = "telephony_enabled"
    private val _prefDeviceName = "device_name"

    @Volatile
    private var lastSdkRestart = 0L
    private val sdkRestartDebounceMs = 10000L // 10 second debounce

    private val networkStateReceiver = object : android.content.BroadcastReceiver() {
        override fun onReceive(context: Context?, intent: Intent?) {
            when (intent?.action) {
                android.bluetooth.BluetoothAdapter.ACTION_STATE_CHANGED -> {
                    val state = intent.getIntExtra(
                        android.bluetooth.BluetoothAdapter.EXTRA_STATE,
                        android.bluetooth.BluetoothAdapter.ERROR
                    )
                    when (state) {
                        android.bluetooth.BluetoothAdapter.STATE_OFF -> {
                            Log.d("ConnectedApp", "Bluetooth turned off - clearing device cache")
                            // Do not stop manager; let it persist for Wi-Fi Direct
                            runOnMainThread {
                                devices.clear()
                                trustedDevices.clear()
                            }
                        }

                        android.bluetooth.BluetoothAdapter.STATE_ON -> {
                            Log.d("ConnectedApp", "Bluetooth turned on - refreshing")
                            scope.launch(Dispatchers.Main) {
                                delay(1000)
                                startProximityManager() // Ensures instance exists
                                if (hasProximityPermissions()) {
                                    try {
                                        proximityManager?.start() // Triggers startBle() again
                                    } catch (e: SecurityException) {
                                        Log.w("ConnectedApp", "Failed to start proximity manager: permission denied", e)
                                    }
                                }
                                beginDiscovery()
                            }
                        }
                    }
                }

                android.net.wifi.WifiManager.WIFI_STATE_CHANGED_ACTION -> {
                    val state = intent.getIntExtra(
                        android.net.wifi.WifiManager.EXTRA_WIFI_STATE,
                        android.net.wifi.WifiManager.WIFI_STATE_UNKNOWN
                    )
                    when (state) {
                        android.net.wifi.WifiManager.WIFI_STATE_DISABLED -> {
                            Log.d("ConnectedApp", "Wi-Fi turned off - clearing device cache")
                            // Do not stop manager; let it persist for BLE
                            runOnMainThread {
                                devices.clear()
                                trustedDevices.clear()
                            }
                        }

                        android.net.wifi.WifiManager.WIFI_STATE_ENABLED -> {
                            Log.d("ConnectedApp", "Wi-Fi turned on - restarting SDK")
                            scope.launch(Dispatchers.Main) {
                                delay(2000)
                                restartSdk()
                            }
                        }
                    }
                }
            }
        }
    }

    fun restartSdk() {
        val now = System.currentTimeMillis()
        if (now - lastSdkRestart < sdkRestartDebounceMs) {
            Log.d("ConnectedApp", "Skipping SDK restart (debounced)")
            return
        }
        lastSdkRestart = now
        Log.d("ConnectedApp", "Restarting SDK to bind to new network interface...")
        cleanup()
        initialize()
    }

    /** Lightweight discovery refresh: clears stale devices and re-announces on mDNS. */
    fun refreshDeviceDiscovery() {
        // Clear UI list immediately so the user sees the refresh
        runOnMainThread {
            devices.clear()
            trustedDevices.clear()
        }
        scope.launch(Dispatchers.IO) {
            try {
                Log.d("ConnectedApp", "Refreshing discovery...")
                refreshDiscovery()
                // Re-fetch whatever the core already knows after a short delay
                // to let the mDNS browse loop pick up responses
                kotlinx.coroutines.delay(500)
                val list = getDiscoveredDevices()
                runOnMainThread {
                    devices.clear()
                    devices.addAll(list)
                    trustedDevices.clear()
                    list.forEach { if (isDeviceTrusted(it)) trustedDevices.add(it.id) }
                }
            } catch (e: Exception) {
                Log.e("ConnectedApp", "Failed to refresh discovery", e)
            }
        }
    }

    fun getDeviceName(): String {
        val prefs = context.getSharedPreferences(_prefsName, Context.MODE_PRIVATE)
        val customName = prefs.getString(_prefDeviceName, null)
        if (customName != null) return customName

        val manufacturer = android.os.Build.MANUFACTURER
        val model = android.os.Build.MODEL
        if (model.startsWith(manufacturer, ignoreCase = true)) {
            return model.replaceFirstChar { it.uppercase() }
        }
        return "${manufacturer.replaceFirstChar { it.uppercase() }} $model"
    }

    fun renameDevice(newName: String) {
        val prefs = context.getSharedPreferences(_prefsName, Context.MODE_PRIVATE)
        prefs.edit { putString(_prefDeviceName, newName) }
        cleanup()
        initialize()
        android.widget.Toast.makeText(context, "Device renamed to $newName", android.widget.Toast.LENGTH_SHORT).show()
    }

    // Renamed wrappers to avoid conflict with imported FFI functions
    fun beginDiscovery() {
        try {
            startDiscovery(discoveryCallback)
        } catch (e: Exception) {
            Log.e("ConnectedApp", "Start discovery failed", e)
        }
    }

    fun startProximityManager() {
        if (proximityManager != null) return
        proximityManager = ProximityManager(context).also { manager ->
            manager.onPairingIntent = { deviceId ->
                if (!pendingPairing.contains(deviceId)) {
                    try {
                        setPairingMode(true)
                    } catch (e: Exception) {
                        Log.w("ConnectedApp", "Failed to enable pairing mode", e)
                    }
                    pendingPairingAwaitingIp.add(deviceId)
                    runOnMainThread {
                        android.widget.Toast.makeText(
                            context,
                            "Connecting to device...",
                            android.widget.Toast.LENGTH_SHORT
                        ).show()
                    }
                }
            }
            manager.hasIdeallyDiscoveredDevice = { deviceId ->
                devices.any { it.id == deviceId && !isSyntheticIp(it.ip) }
            }
            if (hasProximityPermissions()) {
                try {
                    manager.start()
                } catch (e: SecurityException) {
                    Log.w("ConnectedApp", "Failed to start proximity manager: permission denied", e)
                }
            }
        }
    }

    fun stopProximityManager() {
        if (hasProximityPermissions()) {
            try {
                proximityManager?.stop()
            } catch (e: SecurityException) {
                Log.w("ConnectedApp", "Failed to stop proximity manager: permission denied", e)
            }
        }
        proximityManager = null
    }

    private val discoveryCallback = object : DiscoveryCallback {

        override fun onDeviceFound(device: DiscoveredDevice) {

            runOnMainThread {

                val existingIndex = devices.indexOfFirst { it.id == device.id }

                if (existingIndex >= 0) {

                    devices[existingIndex] = device

                } else {

                    val staleIndex = devices.indexOfFirst { it.ip == device.ip && it.port == device.port }

                    if (staleIndex >= 0) devices.removeAt(staleIndex)

                    devices.add(device)

                }



                if (isDeviceTrusted(device)) {
                    locallyUnpairedDevices.remove(device.id)
                    if (!trustedDevices.contains(device.id)) {
                        trustedDevices.add(device.id)
                    }

                    if (pendingPairing.contains(device.id)) {
                        // Core auto-trusted (we initiated). Automatically finalize trusting in UI.
                        pendingPairing.remove(device.id)
                        runOnMainThread {
                            android.widget.Toast.makeText(
                                context,
                                "Paired with ${device.name}",
                                android.widget.Toast.LENGTH_SHORT
                            ).show()
                        }
                    }
                } else {
                    if (trustedDevices.contains(device.id)) {
                        trustedDevices.remove(device.id)
                    }
                }



                if (!isSyntheticIp(device.ip) && pendingPairingAwaitingIp.remove(device.id)) {

                    sendPairRequest(device)

                }



                if (!isSyntheticIp(device.ip)) {

                    pendingFileTransfersAwaitingIp.remove(device.id)?.let { (_, queue) ->

                        var nextPath = queue.poll()

                        while (nextPath != null) {

                            // Use proper Uri encoding to handle special characters in paths
                            val fileUri = Uri.fromFile(File(nextPath)).toString()
                            sendFileToDevice(device, fileUri)

                            nextPath = queue.poll()

                        }

                    }

                }



                if (isDeviceTrusted(device)) {

                    sendLastMediaStateIfAvailable(device)

                }

            }

        }


        override fun onDeviceLost(deviceId: String) {

            runOnMainThread {

                devices.removeAll { it.id == deviceId }

                trustedDevices.remove(deviceId)

                pendingPairingAwaitingIp.remove(deviceId)

                pendingFileTransfersAwaitingIp.remove(deviceId)

            }

        }


        override fun onError(errorMsg: String) {

            Log.e("ConnectedApp", "Discovery error: $errorMsg")

        }

    }

    private val transferCallback = object : FileTransferCallback {
        override fun onTransferRequest(transferId: String, filename: String, fileSize: ULong, fromDevice: String) {
            val request = TransferRequest(transferId, filename, fileSize, fromDevice)
            transferRequest.value = request
            showTransferNotification(request)
        }

        override fun onTransferStarting(filename: String, totalSize: ULong) {
            transferStatus.value = "Starting transfer: $filename"
            showProgressNotification(filename, 0, totalSize.toLong())
        }

        override fun onTransferProgress(bytesTransferred: ULong, totalSize: ULong) {
            val percent = if (totalSize > 0u) (bytesTransferred.toLong() * 100 / totalSize.toLong()) else 0
            transferStatus.value = "Transferring: $percent%"
            showProgressNotification("Downloading...", bytesTransferred.toLong(), totalSize.toLong())
        }

        override fun onTransferCompleted(filename: String, totalSize: ULong) {
            transferStatus.value = "Completed: $filename"
            moveToDownloads(filename)
            showCompletionNotification(filename)
            scope.launch {
                delay(2000)
                if (transferStatus.value.startsWith("Completed: $filename")) {
                    transferStatus.value = "Idle"
                }
            }
        }

        override fun onTransferFailed(errorMsg: String) {
            transferStatus.value = "Failed: $errorMsg"
            showErrorNotification(errorMsg)
        }

        override fun onTransferCancelled() {
            transferStatus.value = "Cancelled"
            val notificationManager =
                context.getSystemService(Context.NOTIFICATION_SERVICE) as android.app.NotificationManager
            notificationManager.cancel(NOTIFICATION_ID_PROGRESS)
        }

        override fun onCompressionProgress(
            filename: String,
            currentFile: String,
            filesProcessed: ULong,
            totalFiles: ULong,
            bytesProcessed: ULong,
            totalBytes: ULong,
            speedBytesPerSec: ULong
        ) {
            val percent = if (totalFiles > 0u) (filesProcessed.toLong() * 100 / totalFiles.toLong()) else 0
            compressionProgress.value = CompressionProgress(
                currentFile = currentFile,
                filesProcessed = filesProcessed.toInt(),
                totalFiles = totalFiles.toInt(),
                bytesProcessed = bytesProcessed.toLong(),
                totalBytes = totalBytes.toLong(),
                speedBytesPerSec = speedBytesPerSec.toLong()
            )
            transferStatus.value = "Compressing: $percent% ($currentFile)"
        }
    }

    private val clipboardCallback = object : ClipboardCallback {
        override fun onClipboardReceived(text: String, fromDevice: String) {
            lastRemoteClipboard.set(text)
            clipboardContent.value = text
            copyToClipboard(text)
            runOnMainThread {
                android.widget.Toast.makeText(
                    context,
                    "Clipboard received from $fromDevice",
                    android.widget.Toast.LENGTH_SHORT
                ).show()
            }
        }

        override fun onClipboardSent(success: Boolean, errorMsg: String?) {
            if (!success) {
                runOnMainThread {
                    android.widget.Toast.makeText(
                        context,
                        "Send failed: $errorMsg",
                        android.widget.Toast.LENGTH_LONG
                    ).show()
                }
            } else {
                runOnMainThread {
                    android.widget.Toast.makeText(
                        context,
                        "Clipboard sent successfully",
                        android.widget.Toast.LENGTH_SHORT
                    ).show()
                }
            }
        }
    }

    private fun copyToClipboard(text: String) {
        runOnMainThread {
            try {
                val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as android.content.ClipboardManager
                val clip = android.content.ClipData.newPlainText("Connected", text)
                clipboard.setPrimaryClip(clip)
            } catch (_: Exception) {
            }
        }
    }

    private val mediaCallback = object : MediaControlCallback {
        override fun onMediaCommand(fromDevice: String, command: MediaCommand) {
            runOnMainThread {
                val wasActive = mediaSession?.isActive == true
                if (wasActive) mediaSession?.isActive = false
                val audioManager = context.getSystemService(Context.AUDIO_SERVICE) as android.media.AudioManager
                val keyEvent = when (command) {
                    MediaCommand.PLAY -> android.view.KeyEvent.KEYCODE_MEDIA_PLAY
                    MediaCommand.PAUSE -> android.view.KeyEvent.KEYCODE_MEDIA_PAUSE
                    MediaCommand.PLAY_PAUSE -> android.view.KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE
                    MediaCommand.NEXT -> android.view.KeyEvent.KEYCODE_MEDIA_NEXT
                    MediaCommand.PREVIOUS -> android.view.KeyEvent.KEYCODE_MEDIA_PREVIOUS
                    MediaCommand.STOP -> android.view.KeyEvent.KEYCODE_MEDIA_STOP
                    MediaCommand.VOLUME_UP -> {
                        audioManager.adjustVolume(
                            android.media.AudioManager.ADJUST_RAISE,
                            android.media.AudioManager.FLAG_SHOW_UI
                        )
                        null
                    }

                    MediaCommand.VOLUME_DOWN -> {
                        audioManager.adjustVolume(
                            android.media.AudioManager.ADJUST_LOWER,
                            android.media.AudioManager.FLAG_SHOW_UI
                        )
                        null
                    }
                }
                if (keyEvent != null) {
                    val down = android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN, keyEvent)
                    val up = android.view.KeyEvent(android.view.KeyEvent.ACTION_UP, keyEvent)
                    audioManager.dispatchMediaKeyEvent(down)
                    audioManager.dispatchMediaKeyEvent(up)
                }
                if (wasActive) mediaSession?.isActive = true
            }
        }

        override fun onMediaStateUpdate(fromDevice: String, state: MediaState) {
            lastMediaSourceDevice = fromDevice
            currentMediaTitle.value = state.title ?: "Unknown Title"
            currentMediaArtist.value = state.artist ?: "Unknown Artist"
            currentMediaPlaying.value = state.playing
            if (isMediaControlEnabled.value) {
                runOnMainThread {
                    initMediaSession()
                    updateMediaSession(state)
                    updateMediaNotification(state)
                }
            }
        }
    }

    private val pairingCallback = object : PairingCallback {
        override fun onPairingRequest(deviceName: String, fingerprint: String, deviceId: String) {
            // Check if Core already trusts this device (e.g. re-connection after local unpair)
            if (isDeviceTrusted(DiscoveredDevice(deviceId, deviceName, "0.0.0.0", 0u, "Unknown"))) {
                Log.d("ConnectedApp", "Already trusted device re-connecting: $deviceName")
                runOnMainThread {
                    android.widget.Toast.makeText(
                        context,
                        "Reconnected with $deviceName",
                        android.widget.Toast.LENGTH_SHORT
                    ).show()
                }
                // Ensure we send trust confirmation so the other side finishes pairing
                trustDevice(PairingRequest(deviceName, fingerprint, deviceId))
                return
            }

            if (pendingPairing.contains(deviceId)) {
                Log.d("ConnectedApp", "Auto-trusting pending device: $deviceName ($deviceId)")
                trustDevice(PairingRequest(deviceName, fingerprint, deviceId))
            } else {
                pairingRequest.value = PairingRequest(deviceName, fingerprint, deviceId)
            }
        }

        override fun onPairingModeChanged(enabled: Boolean) {
            Log.d("ConnectedApp", "Pairing mode changed: $enabled")
        }

        override fun onPairingRejected(deviceName: String, deviceId: String) {
            runOnMainThread {
                pendingPairing.remove(deviceId)
                pendingPairingAwaitingIp.remove(deviceId)
                android.widget.Toast.makeText(
                    context,
                    "Pairing rejected by $deviceName",
                    android.widget.Toast.LENGTH_LONG
                ).show()
            }
        }
    }

    private val unpairCallback = object : UnpairCallback {
        override fun onDeviceUnpaired(deviceId: String, deviceName: String, reason: String) {
            runOnMainThread {
                trustedDevices.remove(deviceId)
                val reasonText = when (reason) {
                    "blocked" -> "blocked you"
                    "forgotten" -> "forgot you"
                    else -> "unpaired from you"
                }
                unpairNotification.value = "$deviceName $reasonText"
            }
        }
    }

    fun initialize() {
        try {
            // Recreate coroutine scope if it was cancelled during cleanup
            if (scopeJob.isCancelled) {
                scopeJob = kotlinx.coroutines.SupervisorJob()
                scope = kotlinx.coroutines.CoroutineScope(Dispatchers.IO + scopeJob)
            }

            val wifiManager =
                context.applicationContext.getSystemService(Context.WIFI_SERVICE) as android.net.wifi.WifiManager
            multicastLock = wifiManager.createMulticastLock("ConnectedMulticastLock")
            multicastLock?.setReferenceCounted(true)
            multicastLock?.acquire()

            downloadDir = File(context.getExternalFilesDir(null), "downloads")
            if (!downloadDir.exists()) downloadDir.mkdirs()

            val storagePath = context.getExternalFilesDir(null)?.absolutePath ?: ""

            try {
                initialize(getDeviceName(), "Mobile", 0u.toUShort(), storagePath)
            } catch (e: Exception) {
                Log.w("ConnectedApp", "Core might be already initialized: ${e.message}")
            }

            startDiscovery(discoveryCallback)
            registerTransferCallback(transferCallback)
            registerClipboardReceiver(clipboardCallback)
            registerPairingCallback(pairingCallback)
            registerUnpairCallback(unpairCallback)
            registerMediaControlCallback(mediaCallback)

            val prefs = context.getSharedPreferences(_prefsName, Context.MODE_PRIVATE)
            if (prefs.getBoolean(_prefTelephonyEnabled, false)) {
                isTelephonyEnabled.value = true
                telephonyProvider.setListener(telephonyListener)
                telephonyProvider.registerReceivers()
                registerTelephonyCallback(telephonyCallback)
            }

            startProximityManager()

            // Stamp the debounce clock BEFORE registering the receiver so that
            // the sticky WIFI_STATE_CHANGED_ACTION broadcast (Android delivers it
            // immediately with the current state) doesn't trigger a spurious
            // restartSdk() right after we just finished initializing.
            lastSdkRestart = System.currentTimeMillis()

            val filter = IntentFilter().apply {
                addAction(android.bluetooth.BluetoothAdapter.ACTION_STATE_CHANGED)
                addAction(android.net.wifi.WifiManager.WIFI_STATE_CHANGED_ACTION)
            }
            context.registerReceiver(networkStateReceiver, filter)

            getPersistedRootUri()?.let { registerFsProvider(it) }

            isMediaControlEnabled.value = prefs.getBoolean(_prefMediaControl, false)


            // Start periodic cleanup of stale pending file transfers (5 minute timeout)
            scope.launch {
                while (true) {
                    kotlinx.coroutines.delay(60_000) // Check every minute
                    val cutoff = System.currentTimeMillis() - 300_000 // 5 minute timeout
                    pendingFileTransfersAwaitingIp.entries.removeIf { (_, pair) ->
                        val (timestamp, _) = pair
                        timestamp < cutoff
                    }
                }
            }
        } catch (e: Exception) {
            Log.e("ConnectedApp", "Initialization failed", e)
        }
    }

    fun cleanup() {
        Log.d("ConnectedApp", "Cleaning up resources")

        // Cancel all coroutines first to stop ongoing operations
        try {
            scopeJob.cancel()
        } catch (e: Exception) {
            Log.w("ConnectedApp", "Error cancelling scope: ${e.message}")
        }

        // Unregister broadcast receiver
        try {
            context.unregisterReceiver(networkStateReceiver)
        } catch (_: Exception) {
            // Already unregistered or never registered
        }

        // Stop clipboard sync job
        stopClipboardSync()

        // Stop proximity manager (BLE + Wi-Fi Direct)
        stopProximityManager()

        // Clear thumbnail cache to free memory
        clearThumbnailCache()

        // Shutdown core SDK
        try {
            shutdown()
        } catch (e: Exception) {
            Log.w("ConnectedApp", "Error during SDK shutdown: ${e.message}")
        }

        // Release multicast lock
        try {
            if (multicastLock?.isHeld == true) {
                multicastLock?.release()
            }
        } catch (_: Exception) {
            // Already released
        }
        multicastLock = null

        // Release media session
        try {
            mediaSession?.release()
        } catch (_: Exception) {
        }
        mediaSession = null

        // Unregister telephony receivers if enabled
        if (isTelephonyEnabled.value) {
            try {
                telephonyProvider.unregisterReceivers()
            } catch (_: Exception) {
            }
        }

        Log.d("ConnectedApp", "Cleanup completed")
    }

    fun runOnMainThread(action: () -> Unit) {
        android.os.Handler(android.os.Looper.getMainLooper()).post(action)
    }

    private fun showTransferNotification(request: TransferRequest) {
        val notificationManager =
            context.getSystemService(Context.NOTIFICATION_SERVICE) as android.app.NotificationManager
        val channelId = "connected_transfer_channel"
        val channel = android.app.NotificationChannel(
            channelId,
            "File Transfers",
            android.app.NotificationManager.IMPORTANCE_HIGH
        )
        notificationManager.createNotificationChannel(channel)

        val acceptIntent = Intent(context, TransferActionReceiver::class.java).apply {
            action = "com.connected.app.ACTION_ACCEPT_TRANSFER"
            putExtra("transferId", request.id)
        }
        val acceptPendingIntent = android.app.PendingIntent.getBroadcast(
            context,
            request.id.hashCode(),
            acceptIntent,
            android.app.PendingIntent.FLAG_UPDATE_CURRENT or android.app.PendingIntent.FLAG_IMMUTABLE
        )

        val rejectIntent = Intent(context, TransferActionReceiver::class.java).apply {
            action = "com.connected.app.ACTION_REJECT_TRANSFER"
            putExtra("transferId", request.id)
        }
        val rejectPendingIntent = android.app.PendingIntent.getBroadcast(
            context,
            request.id.hashCode() + 1,
            rejectIntent,
            android.app.PendingIntent.FLAG_UPDATE_CURRENT or android.app.PendingIntent.FLAG_IMMUTABLE
        )

        val notification = androidx.core.app.NotificationCompat.Builder(context, channelId)
            .setContentTitle("Incoming File")
            .setContentText("${request.fromDevice} wants to send ${request.filename}")
            .setSmallIcon(android.R.drawable.stat_sys_download)
            .setPriority(androidx.core.app.NotificationCompat.PRIORITY_HIGH)
            .addAction(android.R.drawable.ic_menu_add, "Accept", acceptPendingIntent)
            .addAction(android.R.drawable.ic_menu_close_clear_cancel, "Reject", rejectPendingIntent)
            .setAutoCancel(true)
            .build()

        notificationManager.notify(NOTIFICATION_ID_REQUEST, notification)
    }

    private fun showProgressNotification(title: String, current: Long, total: Long) {
        val notificationManager =
            context.getSystemService(Context.NOTIFICATION_SERVICE) as android.app.NotificationManager
        val channelId = "connected_transfer_channel"
        val builder = androidx.core.app.NotificationCompat.Builder(context, channelId)
            .setContentTitle(title)
            .setSmallIcon(android.R.drawable.stat_sys_download)
            .setPriority(androidx.core.app.NotificationCompat.PRIORITY_LOW)
            .setOngoing(true)
            .setOnlyAlertOnce(true)

        if (total > 0) {
            builder.setProgress(total.toInt(), current.toInt(), false)
            val percent = (current * 100 / total).toInt()
            builder.setContentText("$percent%")
        } else {
            builder.setProgress(0, 0, true)
        }
        notificationManager.notify(NOTIFICATION_ID_PROGRESS, builder.build())
    }

    private fun showCompletionNotification(filename: String) {
        val notificationManager =
            context.getSystemService(Context.NOTIFICATION_SERVICE) as android.app.NotificationManager
        notificationManager.cancel(NOTIFICATION_ID_PROGRESS)
        val channelId = "connected_transfer_channel"
        val notification = androidx.core.app.NotificationCompat.Builder(context, channelId)
            .setContentTitle("Download Complete")
            .setContentText(filename)
            .setSmallIcon(android.R.drawable.stat_sys_download_done)
            .setPriority(androidx.core.app.NotificationCompat.PRIORITY_DEFAULT)
            .setAutoCancel(true)
            .build()
        notificationManager.notify(NOTIFICATION_ID_COMPLETE, notification)
    }

    private fun showErrorNotification(error: String) {
        val notificationManager =
            context.getSystemService(Context.NOTIFICATION_SERVICE) as android.app.NotificationManager
        notificationManager.cancel(NOTIFICATION_ID_PROGRESS)
        val channelId = "connected_transfer_channel"
        val notification = androidx.core.app.NotificationCompat.Builder(context, channelId)
            .setContentTitle("Download Failed")
            .setContentText(error)
            .setSmallIcon(android.R.drawable.stat_notify_error)
            .setPriority(androidx.core.app.NotificationCompat.PRIORITY_DEFAULT)
            .setAutoCancel(true)
            .build()
        notificationManager.notify(NOTIFICATION_ID_COMPLETE, notification)
    }

    private fun moveToDownloads(filename: String): Uri? {
        val sourceFile = File(downloadDir, filename)
        if (!sourceFile.exists()) return null
        try {
            val contentValues = android.content.ContentValues().apply {
                put(android.provider.MediaStore.MediaColumns.DISPLAY_NAME, filename)
                put(android.provider.MediaStore.MediaColumns.MIME_TYPE, getMimeType(filename))
                if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.Q) {
                    put(
                        android.provider.MediaStore.MediaColumns.RELATIVE_PATH,
                        android.os.Environment.DIRECTORY_DOWNLOADS
                    )
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
                val outputStream = resolver.openOutputStream(itemUri)
                if (outputStream == null) {
                    resolver.delete(itemUri, null, null)
                    return null
                }
                outputStream.use { output ->
                    sourceFile.inputStream().use { input -> input.copyTo(output) }
                }
                if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.Q) {
                    contentValues.clear()
                    contentValues.put(android.provider.MediaStore.MediaColumns.IS_PENDING, 0)
                    resolver.update(itemUri, contentValues, null, null)
                }
                runOnMainThread {
                    android.widget.Toast.makeText(
                        context,
                        "Saved to Downloads: $filename",
                        android.widget.Toast.LENGTH_LONG
                    ).show()
                }
                return itemUri
            }
        } catch (e: Exception) {
            Log.e("ConnectedApp", "Failed to save to Downloads", e)
        }
        return null
    }

    private fun getMimeType(url: String): String {
        val ext = android.webkit.MimeTypeMap.getFileExtensionFromUrl(url)
        return if (ext != null) android.webkit.MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext)
            ?: "*/*" else "*/*"
    }

    private fun moveFolderToDownloads(folderName: String) {
        val sourceFolder = File(downloadDir, folderName)
        if (!sourceFolder.exists() || !sourceFolder.isDirectory) return

        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                // On Android 10+, use MediaStore for each file
                copyFolderToDownloadsMediaStore(sourceFolder, folderName)
            } else {
                // On older Android, use direct file access
                val downloadsDir = android.os.Environment.getExternalStoragePublicDirectory(
                    android.os.Environment.DIRECTORY_DOWNLOADS
                )
                val destFolder = File(downloadsDir, folderName)
                copyFolderRecursively(sourceFolder, destFolder)
            }

            // Clean up the source folder from app's private storage
            sourceFolder.deleteRecursively()

            Log.i("ConnectedApp", "Moved folder to Downloads: $folderName")
        } catch (e: Exception) {
            Log.e("ConnectedApp", "Failed to move folder to Downloads", e)
        }
    }

    @RequiresApi(Build.VERSION_CODES.Q)
    private fun copyFolderToDownloadsMediaStore(sourceFolder: File, relativePath: String) {
        val resolver = context.contentResolver

        sourceFolder.listFiles()?.forEach { file ->
            if (file.isDirectory) {
                // Recursively handle subdirectories
                copyFolderToDownloadsMediaStore(file, "$relativePath/${file.name}")
            } else {
                // Copy file using MediaStore
                val contentValues = android.content.ContentValues().apply {
                    put(android.provider.MediaStore.MediaColumns.DISPLAY_NAME, file.name)
                    put(android.provider.MediaStore.MediaColumns.MIME_TYPE, getMimeType(file.name))
                    put(
                        android.provider.MediaStore.MediaColumns.RELATIVE_PATH,
                        "${android.os.Environment.DIRECTORY_DOWNLOADS}/$relativePath"
                    )
                    put(android.provider.MediaStore.MediaColumns.IS_PENDING, 1)
                }

                val uri = resolver.insert(
                    android.provider.MediaStore.Downloads.EXTERNAL_CONTENT_URI,
                    contentValues
                )

                if (uri != null) {
                    resolver.openOutputStream(uri)?.use { output ->
                        file.inputStream().use { input ->
                            input.copyTo(output)
                        }
                    }
                    contentValues.clear()
                    contentValues.put(android.provider.MediaStore.MediaColumns.IS_PENDING, 0)
                    resolver.update(uri, contentValues, null, null)
                }
            }
        }
    }

    private fun copyFolderRecursively(source: File, dest: File) {
        if (source.isDirectory) {
            if (!dest.exists()) {
                dest.mkdirs()
            }
            source.listFiles()?.forEach { child ->
                copyFolderRecursively(child, File(dest, child.name))
            }
        } else {
            // Copy file
            source.inputStream().use { input ->
                dest.outputStream().use { output ->
                    input.copyTo(output)
                }
            }
        }
    }

    private fun getPersistedRootUri(): Uri? {
        val prefs = context.getSharedPreferences(_prefsName, Context.MODE_PRIVATE)
        val uriString = prefs.getString(_prefRootUri, null) ?: return null
        return uriString.toUri()
    }

    fun registerFsProvider(uri: Uri) {
        try {
            val provider = AndroidFilesystemProvider(context, uri)
            registerFilesystemProvider(provider)
            isFsProviderRegistered.value = true
            sharedFolderName.value =
                if (uri.scheme == "file") "Full Device Access" else androidx.documentfile.provider.DocumentFile.fromTreeUri(
                    context,
                    uri
                )?.name ?: uri.path
        } catch (_: Exception) {
        }
    }

    fun isSyntheticIp(ip: String) = ip == "0.0.0.0" || ip.startsWith("198.18.")

    fun getDevices() {
        try {
            val list = getDiscoveredDevices()
            devices.clear()
            devices.addAll(list)
            trustedDevices.clear()
            list.forEach { if (isDeviceTrusted(it)) trustedDevices.add(it.id) }
        } catch (_: Exception) {
        }
    }

    fun getRealPathFromUri(contentUri: String): String? {
        try {
            val uri = contentUri.toUri()
            if (uri.scheme == "file") {
                return uri.path
            }
            if (uri.scheme == "content") {
                // Try direct filesystem path when available, otherwise materialize to cache
                context.contentResolver.query(uri, null, null, null, null)?.use {
                    val nameIndex =
                        it.getColumnIndex(android.provider.MediaStore.Files.FileColumns.DATA)
                    if (nameIndex >= 0) {
                        it.moveToFirst()
                        val dataPath = it.getString(nameIndex)
                        if (!dataPath.isNullOrBlank()) {
                            return dataPath
                        }
                    }
                }
                val displayName = queryDisplayName(uri)
                val baseName = displayName ?: uri.lastPathSegment ?: "shared_${UUID.randomUUID()}"
                val safeName = baseName.replace(Regex("[\\\\/:*?\"<>|]"), "_")
                val tempFile =
                    File(context.cacheDir, "shared-${UUID.randomUUID()}-$safeName")
                val input = context.contentResolver.openInputStream(uri) ?: return null
                input.use { stream ->
                    tempFile.outputStream().use { output -> stream.copyTo(output) }
                }
                return tempFile.absolutePath
            }
        } catch (_: Exception) {
        }
        return null
    }

    private fun queryDisplayName(uri: Uri): String? {
        return try {
            context.contentResolver.query(
                uri,
                arrayOf(OpenableColumns.DISPLAY_NAME),
                null,
                null,
                null
            )?.use { cursor ->
                if (cursor.moveToFirst()) {
                    cursor.getString(0)
                } else {
                    null
                }
            }
        } catch (_: Exception) {
            null
        }
    }

    fun copyDocumentFileToLocal(source: androidx.documentfile.provider.DocumentFile, dest: File): Boolean {
        if (source.isDirectory) {
            if (!dest.exists() && !dest.mkdirs()) return false
            source.listFiles().forEach { child ->
                val childDest = File(dest, child.name ?: "unknown")
                if (!copyDocumentFileToLocal(child, childDest)) return false
            }
            return true
        } else {
            return try {
                context.contentResolver.openInputStream(source.uri)?.use { input ->
                    dest.outputStream().use { output -> input.copyTo(output) }
                }
                true
            } catch (_: Exception) {
                false
            }
        }
    }

    fun sendPairRequest(device: DiscoveredDevice) {
        if (isSyntheticIp(device.ip)) {
            try {
                setPairingMode(true)
            } catch (e: Exception) {
                Log.w("ConnectedApp", "Failed to enable pairing mode", e)
            }
            pendingPairingAwaitingIp.add(device.id)
            if (!pendingPairing.contains(device.id)) pendingPairing.add(device.id)
            if (hasProximityPermissions()) {
                try {
                    proximityManager?.requestConnect(device.id)
                } catch (e: SecurityException) {
                    Log.w("ConnectedApp", "Failed to request connect: permission denied", e)
                }
            }
            runOnMainThread {
                android.widget.Toast
                    .makeText(context, "Waiting for Wi-Fi Direct connection...", android.widget.Toast.LENGTH_SHORT)
                    .show()
            }
            return
        }
        pairDevice(device.ip, device.port)
        android.widget.Toast.makeText(context, "Pairing request sent", android.widget.Toast.LENGTH_SHORT).show()
        if (!pendingPairing.contains(device.id)) pendingPairing.add(device.id)
    }

    fun setPendingShare(uris: List<Uri>) {
        pendingShareUris.clear()
        pendingShareUris.addAll(uris.map { it.toString() })
    }

    fun clearPendingShare() {
        pendingShareUris.clear()
    }

    fun sendPendingShareToDevice(device: DiscoveredDevice) {
        val items = pendingShareUris.toList()
        if (items.isEmpty()) return
        items.forEach { uri ->
            sendFileToDevice(device, uri)
        }
        pendingShareUris.clear()
    }

    // Missing method: isDeviceTrusted
    fun isDeviceTrusted(device: DiscoveredDevice): Boolean {
        return try {
            isDeviceTrusted(device.id)
        } catch (_: Exception) {
            false
        }
    }

    // Missing method: sendFileToDevice
    fun sendFileToDevice(device: DiscoveredDevice, fileUri: String) {
        val path = getRealPathFromUri(fileUri)
        if (path == null) {
            runOnMainThread {
                android.widget.Toast.makeText(
                    context,
                    "Unsupported or missing file",
                    android.widget.Toast.LENGTH_SHORT
                ).show()
            }
            return
        }
        val file = File(path)
        if (!file.exists() || file.isDirectory) {
            runOnMainThread {
                android.widget.Toast.makeText(
                    context,
                    "Unsupported or missing file",
                    android.widget.Toast.LENGTH_SHORT
                ).show()
            }
            return
        }
        if (isSyntheticIp(device.ip)) {
            val now = System.currentTimeMillis()
            // Update timestamp when adding to existing queue to prevent premature cleanup
            val entry = pendingFileTransfersAwaitingIp.compute(device.id) { _, existing ->
                if (existing != null) {
                    // Update timestamp and reuse existing queue
                    now to existing.second
                } else {
                    // Create new entry
                    now to ConcurrentLinkedQueue()
                }
            }!!
            entry.second.add(file.absolutePath)
            if (hasProximityPermissions()) {
                try {
                    proximityManager?.requestConnect(device.id)
                } catch (e: SecurityException) {
                    Log.w("ConnectedApp", "Failed to request connect: permission denied", e)
                }
            }
            runOnMainThread {
                android.widget.Toast
                    .makeText(
                        context,
                        "Waiting for Wi-Fi Direct connection...",
                        android.widget.Toast.LENGTH_SHORT
                    )
                    .show()
            }
            return
        }
        scope.launch(Dispatchers.IO) {
            try {
                sendFile(device.ip, device.port, file.absolutePath)
            } catch (e: Exception) {
                Log.e("ConnectedApp", "Send file failed", e)
                runOnMainThread {
                    android.widget.Toast.makeText(
                        context,
                        "Failed to send file",
                        android.widget.Toast.LENGTH_SHORT
                    ).show()
                }
            }
        }
    }

    // Missing Media Session methods
    fun initMediaSession() {
        if (mediaSession == null) {
            mediaSession = MediaSessionCompat(context, "ConnectedMediaSession").apply {
                setCallback(object : MediaSessionCompat.Callback() {
                    override fun onPlay() {
                        sendMediaCommand(MediaCommand.PLAY)
                    }

                    override fun onPause() {
                        sendMediaCommand(MediaCommand.PAUSE)
                    }

                    override fun onSkipToNext() {
                        sendMediaCommand(MediaCommand.NEXT)
                    }

                    override fun onSkipToPrevious() {
                        sendMediaCommand(MediaCommand.PREVIOUS)
                    }

                    override fun onStop() {
                        sendMediaCommand(MediaCommand.STOP)
                    }
                })
                isActive = true
            }
        }
    }

    fun updateMediaSession(state: MediaState) {
        val playbackState = if (state.playing) PlaybackStateCompat.STATE_PLAYING else PlaybackStateCompat.STATE_PAUSED
        mediaSession?.setPlaybackState(
            PlaybackStateCompat.Builder()
                .setState(playbackState, PlaybackStateCompat.PLAYBACK_POSITION_UNKNOWN, 1.0f)
                .setActions(PlaybackStateCompat.ACTION_PLAY or PlaybackStateCompat.ACTION_PAUSE or PlaybackStateCompat.ACTION_SKIP_TO_NEXT or PlaybackStateCompat.ACTION_SKIP_TO_PREVIOUS)
                .build()
        )
        mediaSession?.setMetadata(
            MediaMetadataCompat.Builder()
                .putString(MediaMetadataCompat.METADATA_KEY_TITLE, state.title)
                .putString(MediaMetadataCompat.METADATA_KEY_ARTIST, state.artist)
                .putString(MediaMetadataCompat.METADATA_KEY_ALBUM, state.album)
                .putLong(MediaMetadataCompat.METADATA_KEY_DURATION, -1L)
                .build()
        )
    }

    private fun updateMediaNotification(state: MediaState) {
        val notificationManager =
            context.getSystemService(Context.NOTIFICATION_SERVICE) as android.app.NotificationManager
        val channelId = "connected_media_channel"

        // Create channel if it doesn't exist
        val channel = android.app.NotificationChannel(
            channelId,
            "Media Controls",
            android.app.NotificationManager.IMPORTANCE_LOW
        ).apply {
            description = "Control media playback on connected devices"
            setShowBadge(false)
        }
        notificationManager.createNotificationChannel(channel)

        // Create media control pending intents
        val previousIntent = Intent(MediaControlReceiver.ACTION_PREVIOUS).apply {
            setPackage(context.packageName)
        }
        val previousPendingIntent = android.app.PendingIntent.getBroadcast(
            context,
            1,
            previousIntent,
            android.app.PendingIntent.FLAG_UPDATE_CURRENT or android.app.PendingIntent.FLAG_IMMUTABLE
        )

        val playPauseIntent = Intent(MediaControlReceiver.ACTION_PLAY_PAUSE).apply {
            setPackage(context.packageName)
        }
        val playPausePendingIntent = android.app.PendingIntent.getBroadcast(
            context,
            2,
            playPauseIntent,
            android.app.PendingIntent.FLAG_UPDATE_CURRENT or android.app.PendingIntent.FLAG_IMMUTABLE
        )

        val nextIntent = Intent(MediaControlReceiver.ACTION_NEXT).apply {
            setPackage(context.packageName)
        }
        val nextPendingIntent = android.app.PendingIntent.getBroadcast(
            context,
            3,
            nextIntent,
            android.app.PendingIntent.FLAG_UPDATE_CURRENT or android.app.PendingIntent.FLAG_IMMUTABLE
        )

        val title = state.title ?: "Unknown Title"
        val artist = state.artist ?: "Unknown Artist"
        val playPauseIcon = if (state.playing) R.drawable.ic_pause else R.drawable.ic_play

        val notification = androidx.core.app.NotificationCompat.Builder(context, channelId)
            .setContentTitle(title)
            .setContentText(artist)
            .setSmallIcon(R.drawable.ic_music)
            .setPriority(androidx.core.app.NotificationCompat.PRIORITY_LOW)
            .setCategory(androidx.core.app.NotificationCompat.CATEGORY_TRANSPORT)
            .setVisibility(androidx.core.app.NotificationCompat.VISIBILITY_PUBLIC)
            .setShowWhen(false)
            .setOngoing(state.playing)
            .addAction(R.drawable.ic_previous, "Previous", previousPendingIntent)
            .addAction(playPauseIcon, "Play/Pause", playPausePendingIntent)
            .addAction(R.drawable.ic_next, "Next", nextPendingIntent)
            .setStyle(
                androidx.media.app.NotificationCompat.MediaStyle()
                    .setShowActionsInCompactView(0, 1, 2)
                    .setMediaSession(mediaSession?.sessionToken)
            )
            .build()

        notificationManager.notify(MEDIA_NOTIFICATION_ID, notification)
    }

    private fun sendMediaCommand(command: MediaCommand) {
        lastMediaSourceDevice?.let { deviceName ->
            Log.d("ConnectedApp", "Sending media command $command to device $deviceName")
            val device = devices.find { it.name == deviceName }
            if (device != null) {
                try {
                    Log.d("ConnectedApp", "Found device: ${device.name} at ${device.ip}:${device.port}")
                    sendMediaCommand(device.ip, device.port, command)
                } catch (e: Exception) {
                    Log.e("ConnectedApp", "Media command failed", e)
                }
            } else {
                Log.w("ConnectedApp", "Device $deviceName not found in devices list (size=${devices.size})")
            }
        } ?: Log.w("ConnectedApp", "No lastMediaSourceDevice set")
    }

    /**
     * Public method to send media commands to the last device that sent media state.
     * Used by MediaControlReceiver to handle notification button presses.
     */
    fun sendMediaCommandToLastDevice(command: MediaCommand) {
        scope.launch(Dispatchers.IO) {
            sendMediaCommand(command)
        }
    }

    // Missing Telephony methods
    private val telephonyListener = object : TelephonyProvider.TelephonyListener {
        override fun onCallStateChanged(call: FfiActiveCall?) {
            activeCall.value = call
            // Broadcast update to connected devices if trusted
            devices.forEach { device ->
                if (isDeviceTrusted(device)) {
                    try {
                        sendActiveCallUpdate(device.ip, device.port, call)
                    } catch (_: Exception) {
                    }
                }
            }
        }

        override fun onNewSmsReceived(message: FfiSmsMessage) {
            currentMessages.add(message)
            // Broadcast to connected devices if trusted
            devices.forEach { device ->
                if (isDeviceTrusted(device)) {
                    try {
                        notifyNewSms(device.ip, device.port, message)
                    } catch (_: Exception) {
                    }
                }
            }
        }
    }

    fun relayRcsNotification(title: String, body: String, timestampMs: Long) {
        if (!isTelephonyEnabled.value) {
            return
        }
        val threadId = "rcs:${title.trim()}"
        val messageId = "rcs:${timestampMs}:${body.hashCode()}"
        val msg = FfiSmsMessage(
            id = messageId,
            threadId = threadId,
            address = title,
            contactName = title,
            body = body,
            timestamp = timestampMs.toULong(),
            isOutgoing = false,
            isRead = false,
            status = SmsStatus.RECEIVED,
            attachments = emptyList()
        )

        // Broadcast to connected devices if trusted
        devices.forEach { device ->
            if (isDeviceTrusted(device)) {
                try {
                    notifyNewSms(device.ip, device.port, msg)
                } catch (_: Exception) {
                }
            }
        }
    }

    private val telephonyCallback = object : TelephonyCallback {
        override fun onContactsSyncRequest(fromDevice: String, fromIp: String, fromPort: UShort) {
            scope.launch {
                val contacts = telephonyProvider.getContacts()
                try {
                    sendContacts(fromIp, fromPort, contacts)
                } catch (_: Exception) {
                }
            }
        }

        override fun onContactsReceived(fromDevice: String, contacts: List<FfiContact>) {
            runOnMainThread {
                this@ConnectedApp.contacts.clear()
                this@ConnectedApp.contacts.addAll(contacts)
            }
        }

        override fun onConversationsSyncRequest(fromDevice: String, fromIp: String, fromPort: UShort) {
            scope.launch {
                val convos = telephonyProvider.getConversations()
                try {
                    sendConversations(fromIp, fromPort, convos)
                } catch (_: Exception) {
                }
            }
        }

        override fun onConversationsReceived(fromDevice: String, conversations: List<FfiConversation>) {
            runOnMainThread {
                this@ConnectedApp.conversations.clear()
                this@ConnectedApp.conversations.addAll(conversations)
            }
        }

        override fun onMessagesRequest(
            fromDevice: String,
            fromIp: String,
            fromPort: UShort,
            threadId: String,
            limit: UInt
        ) {
            scope.launch {
                val msgs = telephonyProvider.getMessages(threadId, limit.toInt())
                try {
                    sendMessages(fromIp, fromPort, threadId, msgs)
                } catch (_: Exception) {
                }
            }
        }

        override fun onMessagesReceived(fromDevice: String, threadId: String, messages: List<FfiSmsMessage>) {
            runOnMainThread {
                currentMessages.clear()
                currentMessages.addAll(messages)
            }
        }

        override fun onSendSmsRequest(fromDevice: String, fromIp: String, fromPort: UShort, to: String, body: String) {
            val result = telephonyProvider.sendSms(to, body)
            val success = result.isSuccess
            val error = result.exceptionOrNull()?.message
            try {
                sendSmsSendResult(fromIp, fromPort, success, null, error)
            } catch (_: Exception) {
            }
        }

        override fun onSmsSendResult(success: Boolean, messageId: String?, error: String?) {
            runOnMainThread {
                if (success) android.widget.Toast.makeText(context, "SMS Sent", android.widget.Toast.LENGTH_SHORT)
                    .show()
                else android.widget.Toast.makeText(context, "SMS Failed: $error", android.widget.Toast.LENGTH_LONG)
                    .show()
            }
        }

        override fun onNewSms(fromDevice: String, message: FfiSmsMessage) {
            runOnMainThread {
                currentMessages.add(message)
            }
        }

        override fun onCallLogRequest(fromDevice: String, fromIp: String, fromPort: UShort, limit: UInt) {
            scope.launch {
                val log = telephonyProvider.getCallLog(limit.toInt())
                try {
                    sendCallLog(fromIp, fromPort, log)
                } catch (_: Exception) {
                }
            }
        }

        override fun onCallLogReceived(fromDevice: String, entries: List<FfiCallLogEntry>) {
            runOnMainThread {
                callLog.clear()
                callLog.addAll(entries)
            }
        }

        override fun onInitiateCallRequest(fromDevice: String, fromIp: String, fromPort: UShort, number: String) {
            runOnMainThread {
                telephonyProvider.initiateCall(number)
            }
        }

        override fun onCallActionRequest(fromDevice: String, fromIp: String, fromPort: UShort, action: CallAction) {
            runOnMainThread {
                telephonyProvider.performCallAction(action)
            }
        }

        override fun onActiveCallUpdate(fromDevice: String, call: FfiActiveCall?) {
            runOnMainThread { activeCall.value = call }
        }
    }

    fun stopClipboardSync() {
        isClipboardSyncEnabled.value = false
        clipboardSyncJob?.cancel()
        clipboardSyncJob = null
    }

    private fun getClipboardText(): String {
        if (!isAppInForeground.get()) return ""
        var text = ""
        val latch = java.util.concurrent.CountDownLatch(1)
        runOnMainThread {
            try {
                val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as android.content.ClipboardManager
                if (clipboard.hasPrimaryClip()) {
                    text = clipboard.primaryClip?.getItemAt(0)?.text?.toString() ?: ""
                }
            } catch (_: Exception) {
            }
            latch.countDown()
        }
        try {
            latch.await(1, java.util.concurrent.TimeUnit.SECONDS)
        } catch (_: Exception) {
        }
        return text
    }

    // Device Selection for File Transfer
    fun getSelectedDeviceForFileTransfer(): DiscoveredDevice? = selectedDeviceForFile
    fun setSelectedDeviceForFileTransfer(device: DiscoveredDevice?) {
        selectedDeviceForFile = device
    }

    // Settings / Preferences
    fun setRootUri(uri: Uri) {
        val prefs = context.getSharedPreferences(_prefsName, Context.MODE_PRIVATE)
        prefs.edit { putString(_prefRootUri, uri.toString()) }

        // Persist permission
        try {
            context.contentResolver.takePersistableUriPermission(
                uri,
                Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION
            )
        } catch (_: Exception) {
        }

        registerFsProvider(uri)
    }

    fun toggleTelephony() {
        val newState = !isTelephonyEnabled.value
        isTelephonyEnabled.value = newState
        context.getSharedPreferences(_prefsName, Context.MODE_PRIVATE).edit {
            putBoolean(
                _prefTelephonyEnabled,
                newState
            )
        }
        if (newState) {
            telephonyProvider.setListener(telephonyListener)
            telephonyProvider.registerReceivers()
            registerTelephonyCallback(telephonyCallback)
        } else {
            telephonyProvider.setListener(null)
            telephonyProvider.unregisterReceivers()
        }
    }

    fun toggleMediaControl() {
        val newState = !isMediaControlEnabled.value
        isMediaControlEnabled.value = newState
        context.getSharedPreferences(_prefsName, Context.MODE_PRIVATE).edit {
            putBoolean(
                _prefMediaControl,
                newState
            )
        }

        // Clear media session and notification when disabled
        if (!newState) {
            try {
                mediaSession?.release()
                mediaSession = null
            } catch (_: Exception) {
            }

            // Remove media notification
            val notificationManager =
                context.getSystemService(Context.NOTIFICATION_SERVICE) as android.app.NotificationManager
            notificationManager.cancel(MEDIA_NOTIFICATION_ID)

            // Reset media state
            currentMediaTitle.value = "Not Playing"
            currentMediaArtist.value = ""
            currentMediaPlaying.value = false
            lastMediaSourceDevice = null
            lastBroadcastMediaState = null
        }
    }

    fun setAppInForeground(isForeground: Boolean) {
        isAppInForeground.set(isForeground)
    }

    // Media Observer
    fun onLocalMediaUpdate(state: MediaState) {
        if (!isMediaControlEnabled.value) return

        // Check if state changed significantly to avoid spam
        if (lastBroadcastMediaState?.title == state.title && lastBroadcastMediaState?.playing == state.playing) return
        lastBroadcastMediaState = state

        devices.forEach { device ->
            if (isDeviceTrusted(device)) {
                try {
                    sendMediaState(device.ip, device.port, state)
                } catch (_: Exception) {
                }
            }
        }
    }

    // Public Media Command
    fun sendMediaCommand(device: DiscoveredDevice, command: MediaCommand) {
        scope.launch(Dispatchers.IO) {
            try {
                sendMediaCommand(device.ip, device.port, command)
            } catch (e: Exception) {
                Log.e("ConnectedApp", "Media command failed", e)
            }
        }
    }

    private fun sendLastMediaStateIfAvailable(device: DiscoveredDevice) {
        val lastState = lastBroadcastMediaState ?: return
        if (!isMediaControlEnabled.value || isSyntheticIp(device.ip)) return
        scope.launch(Dispatchers.IO) {
            try {
                sendMediaState(device.ip, device.port, lastState)
            } catch (e: Exception) {
                Log.e("ConnectedApp", "Failed to send media state", e)
            }
        }
    }

    // Send Clipboard Manually
    fun sendClipboard(device: DiscoveredDevice) {
        scope.launch {
            if (!isAppInForeground.get()) {
                runOnMainThread {
                    android.widget.Toast.makeText(
                        context,
                        "Open the app to access clipboard on Android 15",
                        android.widget.Toast.LENGTH_SHORT
                    ).show()
                }
                return@launch
            }
            val clip = getClipboardText()
            if (clip.isNotEmpty()) {
                try {
                    sendClipboard(device.ip, device.port, clip, clipboardCallback)
                } catch (_: Exception) {
                    runOnMainThread {
                        android.widget.Toast.makeText(
                            context,
                            "Failed to send clipboard",
                            android.widget.Toast.LENGTH_SHORT
                        ).show()
                    }
                }
            }
        }
    }

    fun broadcastClipboard(text: String) {
        scope.launch {
            devices.forEach { device ->
                if (isDeviceTrusted(device)) {
                    try {
                        sendClipboard(device.ip, device.port, text, clipboardCallback)
                    } catch (e: Exception) {
                        Log.e("ConnectedApp", "Failed to send clipboard to ${device.name}", e)
                    }
                }
            }
            runOnMainThread {
                android.widget.Toast.makeText(
                    context,
                    "Clipboard shared with trusted devices",
                    android.widget.Toast.LENGTH_SHORT
                ).show()
            }
        }
    }

    // Send Clipboard to all trusted devices
    fun sendClipboardToAllTrusted() {
        scope.launch {
            if (!isAppInForeground.get() && android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.Q) {
                // Background clipboard access restriction
                runOnMainThread {
                    android.widget.Toast.makeText(
                        context,
                        "Bring app to foreground to share clipboard",
                        android.widget.Toast.LENGTH_SHORT
                    ).show()
                }
                return@launch
            }
            val clip = getClipboardText()
            if (clip.isNotEmpty()) {
                devices.forEach { device ->
                    if (isDeviceTrusted(device)) {
                        try {
                            sendClipboard(device.ip, device.port, clip, clipboardCallback)
                        } catch (e: Exception) {
                            Log.e("ConnectedApp", "Failed to send clipboard to ${device.name}", e)
                        }
                    }
                }
                runOnMainThread {
                    android.widget.Toast.makeText(
                        context,
                        "Clipboard shared with trusted devices",
                        android.widget.Toast.LENGTH_SHORT
                    ).show()
                }
            } else {
                runOnMainThread {
                    android.widget.Toast.makeText(
                        context,
                        "Clipboard is empty",
                        android.widget.Toast.LENGTH_SHORT
                    ).show()
                }
            }
        }
    }

    // Remote File Browsing
    fun getBrowsingDevice(): DiscoveredDevice? = browsingDevice

    fun browseRemoteFiles(device: DiscoveredDevice, path: String = "/") {
        browsingDevice = device
        currentRemotePath.value = path
        isBrowsingRemote.value = true
        scope.launch(Dispatchers.IO) {
            try {
                val list = requestListDir(device.ip, device.port, path)
                runOnMainThread {
                    remoteFiles.clear()
                    remoteFiles.addAll(list)
                }
            } catch (e: Exception) {
                runOnMainThread {
                    android.widget.Toast.makeText(
                        context,
                        "Failed to browse: ${e.message}",
                        android.widget.Toast.LENGTH_SHORT
                    ).show()
                }
            }
        }
    }

    fun closeRemoteBrowser() {
        isBrowsingRemote.value = false
        browsingDevice = null
        remoteFiles.clear()
        currentRemotePath.value = "/"
        clearThumbnailCache()
    }

    fun downloadRemoteFile(device: DiscoveredDevice, remotePath: String) {
        val fileName = File(remotePath).name
        val destFile = File(downloadDir, fileName)

        // Show initial progress
        browserDownloadProgress.value = BrowserDownloadProgress(
            currentFile = fileName,
            bytesDownloaded = 0,
            totalBytes = 0,
            isFolder = false
        )

        val callback = object : BrowserDownloadCallback {
            override fun onDownloadProgress(bytesDownloaded: ULong, totalBytes: ULong, currentFile: String) {
                runOnMainThread {
                    browserDownloadProgress.value = BrowserDownloadProgress(
                        currentFile = currentFile,
                        bytesDownloaded = bytesDownloaded.toLong(),
                        totalBytes = totalBytes.toLong(),
                        isFolder = false
                    )
                }
            }

            override fun onDownloadCompleted(totalBytes: ULong) {
                runOnMainThread {
                    browserDownloadProgress.value = null
                    moveToDownloads(fileName)
                    android.widget.Toast.makeText(context, "Downloaded $fileName", android.widget.Toast.LENGTH_SHORT)
                        .show()
                }
            }

            override fun onDownloadFailed(errorMsg: String) {
                runOnMainThread {
                    browserDownloadProgress.value = null
                    android.widget.Toast.makeText(
                        context,
                        "Download failed: $errorMsg",
                        android.widget.Toast.LENGTH_SHORT
                    ).show()
                }
            }
        }

        requestDownloadFileWithProgress(device.ip, device.port, remotePath, destFile.absolutePath, callback)
    }

    fun downloadRemoteFolder(device: DiscoveredDevice, remotePath: String) {
        val folderName = File(remotePath).name

        // Show initial progress
        browserDownloadProgress.value = BrowserDownloadProgress(
            currentFile = folderName,
            bytesDownloaded = 0,
            totalBytes = 0,
            isFolder = true
        )

        val callback = object : BrowserDownloadCallback {
            override fun onDownloadProgress(bytesDownloaded: ULong, totalBytes: ULong, currentFile: String) {
                runOnMainThread {
                    browserDownloadProgress.value = BrowserDownloadProgress(
                        currentFile = currentFile,
                        bytesDownloaded = bytesDownloaded.toLong(),
                        totalBytes = totalBytes.toLong(),
                        isFolder = true
                    )
                }
            }

            override fun onDownloadCompleted(totalBytes: ULong) {
                runOnMainThread {
                    browserDownloadProgress.value = null
                    moveFolderToDownloads(folderName)
                    android.widget.Toast.makeText(
                        context,
                        "Downloaded folder $folderName",
                        android.widget.Toast.LENGTH_SHORT
                    ).show()
                }
            }

            override fun onDownloadFailed(errorMsg: String) {
                runOnMainThread {
                    browserDownloadProgress.value = null
                    android.widget.Toast.makeText(
                        context,
                        "Folder download failed: $errorMsg",
                        android.widget.Toast.LENGTH_SHORT
                    ).show()
                }
            }
        }

        // Pass downloadDir - the core will create the folder with its name inside
        requestDownloadFolder(device.ip, device.port, remotePath, downloadDir.absolutePath, callback)
    }

    fun getThumbnail(path: String) {
        // Use single lock for all cache access to prevent race conditions
        synchronized(thumbnailLock) {
            if (requestedThumbnails.contains(path)) return

            // If already cached, update access order for LRU (move to end = most recently used)
            if (thumbnails.containsKey(path)) {
                // Remove from current position and add to end (most recent)
                thumbnailAccessOrder.remove(path)
                thumbnailAccessOrder.add(path)
                return
            }

            requestedThumbnails.add(path)
        }

        val device = browsingDevice ?: run {
            synchronized(thumbnailLock) { requestedThumbnails.remove(path) }
            return
        }

        scope.launch(Dispatchers.IO) {
            try {
                val bytes = requestGetThumbnail(device.ip, device.port, path)
                if (bytes.isNotEmpty()) {
                    val bitmap = BitmapFactory.decodeByteArray(bytes, 0, bytes.size)
                    if (bitmap != null) {
                        runOnMainThread {
                            synchronized(thumbnailLock) {
                                // Evict oldest entries if cache is full (LRU eviction)
                                // Oldest entries are at the beginning of the list
                                while (thumbnails.size >= maxThumbnailCacheSize && thumbnailAccessOrder.isNotEmpty()) {
                                    val oldest = thumbnailAccessOrder.removeAt(0)
                                    thumbnails.remove(oldest)?.recycle()
                                    requestedThumbnails.remove(oldest)
                                }

                                thumbnails[path] = bitmap
                                // Ensure no duplicates before adding
                                thumbnailAccessOrder.remove(path)
                                thumbnailAccessOrder.add(path)
                                requestedThumbnails.remove(path) // Mark as done, not pending
                            }
                        }
                        return@launch
                    }
                }
            } catch (_: Exception) {
                // Fall through to allow retry
            }
            synchronized(thumbnailLock) { requestedThumbnails.remove(path) }
        }
    }

    /**
     * Clear thumbnail cache when browsing a different device or closing browser.
     * Called to free memory.
     */
    private fun clearThumbnailCache() {
        synchronized(thumbnailLock) {
            thumbnailAccessOrder.clear()
            requestedThumbnails.clear()
        }
        runOnMainThread {
            synchronized(thumbnailLock) {
                thumbnails.values.forEach { it.recycle() }
                thumbnails.clear()
            }
        }
    }

    // Folder Transfer (Zipped)

    /**
     * Scans a directory tree and returns (totalFiles, totalBytes)
     */
    private fun scanFolderStats(root: androidx.documentfile.provider.DocumentFile): Pair<Int, Long> {
        var totalFiles = 0
        var totalBytes = 0L
        val stack = ArrayDeque<androidx.documentfile.provider.DocumentFile>()
        stack.addLast(root)

        while (stack.isNotEmpty()) {
            val current = stack.removeLast()
            current.listFiles().forEach { file ->
                if (file.isDirectory) {
                    stack.addLast(file)
                } else {
                    totalFiles++
                    totalBytes += file.length()
                }
            }
        }
        return totalFiles to totalBytes
    }

    fun sendFolderToDevice(device: DiscoveredDevice, folderUri: Uri) {

        scope.launch(Dispatchers.IO) {

            val docFile = androidx.documentfile.provider.DocumentFile.fromTreeUri(context, folderUri)

            if (docFile != null && docFile.isDirectory) {

                val folderName = docFile.name ?: "folder"

                val zipFile = File(context.cacheDir, "$folderName.zip")

                try {
                    // First scan to get total size for progress tracking
                    runOnMainThread {
                        compressionProgress.value = CompressionProgress(
                            currentFile = "Scanning folder...",
                            filesProcessed = 0,
                            totalFiles = 0,
                            bytesProcessed = 0,
                            totalBytes = 0,
                            speedBytesPerSec = 0
                        )
                    }

                    val (totalFiles, totalBytes) = scanFolderStats(docFile)

                    if (totalFiles == 0) {
                        runOnMainThread {
                            compressionProgress.value = null
                            android.widget.Toast.makeText(
                                context,
                                "Folder is empty",
                                android.widget.Toast.LENGTH_SHORT
                            ).show()
                        }
                        return@launch
                    }

                    ZipOutputStream(FileOutputStream(zipFile)).use { zos ->
                        zipRecursiveWithProgress(docFile, folderName, zos, totalFiles, totalBytes)
                    }

                    // Clear compression progress
                    runOnMainThread {
                        compressionProgress.value = null
                    }



                    if (isSyntheticIp(device.ip)) {

                        val now = System.currentTimeMillis()
                        val (_, queue) = pendingFileTransfersAwaitingIp.computeIfAbsent(device.id) {

                            now to ConcurrentLinkedQueue()

                        }

                        queue.add(zipFile.absolutePath)

                        if (hasProximityPermissions()) {
                            try {
                                proximityManager?.requestConnect(device.id)
                            } catch (e: SecurityException) {
                                Log.w("ConnectedApp", "Failed to request connect: permission denied", e)
                            }
                        }

                        runOnMainThread {

                            android.widget.Toast

                                .makeText(

                                    context,

                                    "Waiting for Wi-Fi Direct connection...",

                                    android.widget.Toast.LENGTH_SHORT

                                )

                                .show()

                        }

                    } else {

                        sendFile(device.ip, device.port, zipFile.absolutePath)

                    }

                } catch (e: Exception) {

                    Log.e("ConnectedApp", "Failed to zip folder", e)

                    runOnMainThread {

                        android.widget.Toast.makeText(
                            context,
                            "Failed to zip folder: ${e.message}",
                            android.widget.Toast.LENGTH_SHORT
                        ).show()

                    }

                }

            }

        }

    }


    /**
     * Iterative implementation to zip a directory structure with progress tracking.
     * Uses an explicit stack instead of recursion to avoid stack overflow
     * on deeply nested folder structures.
     */
    private fun zipRecursiveWithProgress(
        root: androidx.documentfile.provider.DocumentFile,
        parentPath: String,
        zos: ZipOutputStream,
        totalFiles: Int,
        totalBytes: Long
    ) {
        // Stack holds pairs of (DocumentFile, parentPath)
        val stack = ArrayDeque<Pair<androidx.documentfile.provider.DocumentFile, String>>()
        stack.addLast(root to parentPath)

        var filesProcessed = 0
        var bytesProcessed = 0L
        val startTime = System.currentTimeMillis()
        var lastProgressUpdate = 0L

        while (stack.isNotEmpty()) {
            val (currentDir, currentParentPath) = stack.removeLast()

            currentDir.listFiles().forEach { file ->
                val entryPath = if (currentParentPath.isEmpty()) {
                    file.name ?: ""
                } else {
                    "$currentParentPath/${file.name}"
                }

                if (file.isDirectory) {
                    // Push directory onto stack for later processing
                    stack.addLast(file to entryPath)
                } else {
                    try {
                        val fileName = file.name ?: "unknown"
                        val fileSize = file.length()

                        val entry = ZipEntry(entryPath)
                        zos.putNextEntry(entry)

                        context.contentResolver.openInputStream(file.uri)?.use { input ->
                            val buffer = ByteArray(8192)
                            var bytesRead: Int
                            while (input.read(buffer).also { bytesRead = it } != -1) {
                                zos.write(buffer, 0, bytesRead)
                                bytesProcessed += bytesRead

                                // Update progress every 100ms
                                val now = System.currentTimeMillis()
                                if (now - lastProgressUpdate >= 100) {
                                    lastProgressUpdate = now
                                    val elapsedMs = now - startTime
                                    val speed = if (elapsedMs > 0) {
                                        (bytesProcessed * 1000 / elapsedMs)
                                    } else 0L

                                    val progress = CompressionProgress(
                                        currentFile = fileName,
                                        filesProcessed = filesProcessed,
                                        totalFiles = totalFiles,
                                        bytesProcessed = bytesProcessed,
                                        totalBytes = totalBytes,
                                        speedBytesPerSec = speed
                                    )
                                    runOnMainThread {
                                        compressionProgress.value = progress
                                    }
                                }
                            }
                        }
                        zos.closeEntry()
                        filesProcessed++

                        // Final update for this file
                        val now = System.currentTimeMillis()
                        val elapsedMs = now - startTime
                        val speed = if (elapsedMs > 0) {
                            (bytesProcessed * 1000 / elapsedMs)
                        } else 0L

                        val progress = CompressionProgress(
                            currentFile = fileName,
                            filesProcessed = filesProcessed,
                            totalFiles = totalFiles,
                            bytesProcessed = bytesProcessed,
                            totalBytes = totalBytes,
                            speedBytesPerSec = speed
                        )
                        runOnMainThread {
                            compressionProgress.value = progress
                        }

                    } catch (e: Exception) {
                        Log.e("ConnectedApp", "Failed to zip file: ${file.name}", e)
                    }
                }
            }
        }
    }

    // Device Management Wrappers
    fun pairDevice(device: DiscoveredDevice) {
        locallyUnpairedDevices.remove(device.id)
        sendPairRequest(device)
    }

    fun unpairDevice(device: DiscoveredDevice) {
        scope.launch(Dispatchers.IO) {
            locallyUnpairedDevices.add(device.id)
            trustedDevices.remove(device.id)
            pendingPairing.remove(device.id)
            try {
                unpairDeviceById(device.id)
                runOnMainThread {
                    android.widget.Toast.makeText(context, "Device unpaired", android.widget.Toast.LENGTH_SHORT).show()
                }
            } catch (e: Exception) {
                Log.e("ConnectedApp", "Unpair failed", e)
                runOnMainThread {
                    android.widget.Toast.makeText(
                        context,
                        "Unpair failed: ${e.message}",
                        android.widget.Toast.LENGTH_LONG
                    ).show()
                }
            }
        }
    }

    fun forgetDevice(device: DiscoveredDevice) {
        scope.launch(Dispatchers.IO) {
            trustedDevices.remove(device.id)
            pendingPairing.remove(device.id)
            try {
                forgetDeviceById(device.id)
                runOnMainThread {
                    // Do not remove from devices list; just update trusted state
                    android.widget.Toast.makeText(context, "Device forgotten", android.widget.Toast.LENGTH_SHORT).show()
                }
            } catch (e: Exception) {
                Log.e("ConnectedApp", "Forget failed", e)
                runOnMainThread {
                    android.widget.Toast.makeText(
                        context,
                        "Forget failed: ${e.message}",
                        android.widget.Toast.LENGTH_LONG
                    ).show()
                }
            }
        }
    }

    fun rejectDevice(request: PairingRequest) {
        scope.launch(Dispatchers.IO) {
            try {
                rejectPairing(request.deviceId)
            } catch (e: Exception) {
                Log.e("ConnectedApp", "Reject pairing failed", e)
            }
            pairingRequest.value = null
        }
    }

    fun trustDevice(request: PairingRequest) {
        scope.launch(Dispatchers.IO) {
            try {
                if (request.fingerprint != "Verified (You initiated)") {
                    trustDevice(request.fingerprint, request.deviceId, request.deviceName)
                }
                val device = devices.find { it.id == request.deviceId }
                if (device != null && !isSyntheticIp(device.ip)) {
                    sendTrustConfirmation(device.ip, device.port)
                }
                pairingRequest.value = null
                runOnMainThread {
                    locallyUnpairedDevices.remove(request.deviceId)
                    pendingPairing.remove(request.deviceId)
                    if (!trustedDevices.contains(request.deviceId)) {
                        trustedDevices.add(request.deviceId)
                    }
                    android.widget.Toast.makeText(context, "Device trusted", android.widget.Toast.LENGTH_SHORT).show()
                }
            } catch (e: Exception) {
                runOnMainThread {
                    android.widget.Toast.makeText(
                        context,
                        "Failed to trust: ${e.message}",
                        android.widget.Toast.LENGTH_SHORT
                    ).show()
                }
            }
        }
    }

    fun acceptTransfer(request: TransferRequest) {
        scope.launch(Dispatchers.IO) {
            try {
                acceptFileTransfer(request.id)
            } catch (_: Exception) {
            }
            transferRequest.value = null
            dismissTransferNotification()
        }
    }

    fun rejectTransfer(request: TransferRequest) {
        scope.launch(Dispatchers.IO) {
            try {
                rejectFileTransfer(request.id)
            } catch (_: Exception) {
            }
            transferRequest.value = null
            dismissTransferNotification()
        }
    }

    fun dismissTransferNotification() {
        val notificationManager =
            context.getSystemService(Context.NOTIFICATION_SERVICE) as android.app.NotificationManager
        notificationManager.cancel(NOTIFICATION_ID_REQUEST)
    }

    // Permission Helpers
    fun isFullAccessGranted(): Boolean {
        return if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.R) {
            android.os.Environment.isExternalStorageManager()
        } else {
            true // Legacy
        }
    }

    fun setFullAccess() {
        if (isFullAccessGranted()) {
            val root = android.os.Environment.getExternalStorageDirectory()
            val uri = Uri.fromFile(root)
            setRootUri(uri)
            runOnMainThread {
                android.widget.Toast.makeText(context, "Full Device Access Enabled", android.widget.Toast.LENGTH_SHORT)
                    .show()
            }
        }
    }

    fun requestFullAccessPermission() {
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.R) {
            try {
                val intent = Intent(android.provider.Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION)
                intent.addCategory("android.intent.category.DEFAULT")
                intent.data = "package:${context.packageName}".toUri()
                intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                context.startActivity(intent)
            } catch (_: Exception) {
                val intent = Intent(android.provider.Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION)
                intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                context.startActivity(intent)
            }
        }
    }

    suspend fun checkForUpdates(): UpdateInfo? {
        return try {
            RustlsPlatformVerifier.initIfNeeded(context)
            val packageInfo = context.packageManager.getPackageInfo(context.packageName, 0)
            val currentVersion = packageInfo.versionName ?: "0.0.0"
            checkForUpdates(currentVersion, "android")
        } catch (e: Exception) {
            Log.e("ConnectedApp", "Failed to check for updates", e)
            null
        }
    }

    fun downloadUpdate(url: String, onProgress: (Int) -> Unit, onComplete: (File?) -> Unit) {
        scope.launch(Dispatchers.IO) {
            try {
                val request = java.net.URL(url)
                val connection = request.openConnection() as java.net.HttpURLConnection
                connection.instanceFollowRedirects = true
                connection.setRequestProperty("User-Agent", "Connected-App")
                connection.connect()

                val fileLength = connection.contentLength
                val input = java.io.BufferedInputStream(connection.inputStream)
                // Use internal cache so FileProvider paths are stable and we don't depend on
                // external storage availability.
                val outputFile = File(context.cacheDir, "connected-update.apk")
                val output = FileOutputStream(outputFile)

                val data = ByteArray(1024)
                var total: Long = 0
                var count: Int
                while (input.read(data).also { count = it } != -1) {
                    total += count.toLong()
                    if (fileLength > 0) {
                        val progress = (total * 100 / fileLength).toInt()
                        runOnMainThread { onProgress(progress) }
                    }
                    output.write(data, 0, count)
                }

                output.flush()
                output.close()
                input.close()

                runOnMainThread { onComplete(outputFile) }
            } catch (e: Exception) {
                Log.e("ConnectedApp", "Update download failed", e)
                runOnMainThread { onComplete(null) }
            }
        }
    }

    fun resumePendingUpdateInstallIfAllowed() {
        val pendingFile = pendingUpdateInstall ?: return
        if (android.os.Build.VERSION.SDK_INT < android.os.Build.VERSION_CODES.O ||
            context.packageManager.canRequestPackageInstalls()
        ) {
            pendingUpdateInstall = null
            installApkInternal(pendingFile)
        }
    }

    fun installApk(file: File) {
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.O &&
            !context.packageManager.canRequestPackageInstalls()
        ) {
            pendingUpdateInstall = file
            val intent = Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES).apply {
                data = Uri.parse("package:${context.packageName}")
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            }
            context.startActivity(intent)
            runOnMainThread {
                android.widget.Toast.makeText(
                    context,
                    "Allow this app to install updates, then return to finish install.",
                    android.widget.Toast.LENGTH_LONG
                ).show()
            }
            return
        }
        installApkInternal(file)
    }

    private fun installApkInternal(file: File) {
        try {
            val uri = androidx.core.content.FileProvider.getUriForFile(
                context,
                "${context.packageName}.provider",
                file
            )
            val intent = Intent(Intent.ACTION_VIEW)
            intent.setDataAndType(uri, "application/vnd.android.package-archive")
            intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            context.startActivity(intent)
        } catch (e: Exception) {
            Log.e("ConnectedApp", "Install APK failed", e)
            runOnMainThread {
                android.widget.Toast.makeText(context, "Install failed: ${e.message}", android.widget.Toast.LENGTH_LONG)
                    .show()
            }
        }
    }
}
