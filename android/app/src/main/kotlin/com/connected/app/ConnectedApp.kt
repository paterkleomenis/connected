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
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.isActive
import kotlinx.coroutines.delay

import android.support.v4.media.session.MediaSessionCompat
import android.support.v4.media.session.PlaybackStateCompat
import android.support.v4.media.MediaMetadataCompat
import android.graphics.BitmapFactory

class ConnectedApp(private val context: Context) {

    companion object {
        private var instance: ConnectedApp? = null
        fun getInstance(): ConnectedApp? = instance
    }

    init {
        instance = this
    }

    // State exposed to Compose
    val devices = mutableStateListOf<DiscoveredDevice>()
    val trustedDevices = mutableStateListOf<String>() // Set of trusted Device IDs
    val pendingPairing = mutableStateListOf<String>() // Set of pending Device IDs

    // Cache of trusted device addresses (IP:Port) for sending notifications
    // Updated whenever we receive a message from a trusted device
    private val trustedDeviceAddresses = mutableMapOf<String, Pair<String, UShort>>() // deviceName -> (ip, port)
    val transferStatus = mutableStateOf("Idle")
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

    data class PairingRequest(val deviceName: String, val fingerprint: String, val deviceId: String)
    data class TransferRequest(val id: String, val filename: String, val fileSize: ULong, val fromDevice: String)

    // Clipboard Sync State
    val isClipboardSyncEnabled = mutableStateOf(false)
    val isMediaControlEnabled = mutableStateOf(false)
    private var clipboardSyncJob: kotlinx.coroutines.Job? = null

    @Volatile
    private var lastLocalClipboard: String = ""

    @Volatile
    private var lastRemoteClipboard: String = ""
    private val scope =
        kotlinx.coroutines.CoroutineScope(kotlinx.coroutines.Dispatchers.IO + kotlinx.coroutines.SupervisorJob())

    // Track when other devices unpair us
    val unpairNotification = mutableStateOf<String?>(null)

    // Store selected device for file transfer
    private var selectedDeviceForFile: DiscoveredDevice? = null
    private lateinit var downloadDir: File

    // FilesystemProvider State
    val isFsProviderRegistered = mutableStateOf(false)
    val sharedFolderName = mutableStateOf<String?>(null)
    val remoteFiles = mutableStateListOf<FfiFsEntry>()
    val currentRemotePath = mutableStateOf("/")
    val isBrowsingRemote = mutableStateOf(false)
    private var browsingDevice: DiscoveredDevice? = null
    val thumbnails = androidx.compose.runtime.mutableStateMapOf<String, android.graphics.Bitmap>()
    private val requestedThumbnails = mutableSetOf<String>()

    // Network State
    private var multicastLock: android.net.wifi.WifiManager.MulticastLock? = null

    private val PREFS_NAME = "ConnectedPrefs"
    private val PREF_ROOT_URI = "root_uri"
    private val PREF_CLIPBOARD_SYNC = "clipboard_sync"
    private val PREF_MEDIA_CONTROL = "media_control"
    private val PREF_TELEPHONY_ENABLED = "telephony_enabled"
    private val PREF_DEVICE_NAME = "device_name"

    fun getDeviceName(): String {
        val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        val customName = prefs.getString(PREF_DEVICE_NAME, null)
        if (customName != null) return customName

        val manufacturer = android.os.Build.MANUFACTURER
        val model = android.os.Build.MODEL
        if (model.startsWith(manufacturer, ignoreCase = true)) {
            return model.replaceFirstChar { it.uppercase() }
        }
        return "${manufacturer.replaceFirstChar { it.uppercase() }} $model"
    }

    fun renameDevice(newName: String) {
        val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        prefs.edit().putString(PREF_DEVICE_NAME, newName).apply()

        // Restart service to apply new name
        cleanup()
        initialize()

        android.widget.Toast.makeText(context, "Device renamed to $newName", android.widget.Toast.LENGTH_SHORT).show()
    }

    // Helper to find device by name or fall back to first trusted device
    private fun findDeviceByName(fromDevice: String): DiscoveredDevice? {
        Log.d("ConnectedApp", "Looking for device: '$fromDevice' in ${devices.size} devices")
        devices.forEach {
            val isTrusted = trustedDevices.contains(it.id)
            Log.d(
                "ConnectedApp",
                "  - Device: name='${it.name}', id='${it.id}', trusted=$isTrusted"
            )
        }

        // Try exact name match first
        var device = devices.find { it.name == fromDevice }

        // If not found, try to find any trusted device
        if (device == null) {
            Log.d("ConnectedApp", "Device not found by name, trying trusted devices")
            device = devices.find { trustedDevices.contains(it.id) }
        }

        if (device != null) {
            Log.d("ConnectedApp", "Found device: ${device.name} (${device.ip}:${device.port})")
        } else {
            Log.w("ConnectedApp", "No device found for '$fromDevice'")
        }

        return device
    }

    // Telephony Callback implementation
    private val telephonyCallback = object : TelephonyCallback {
        override fun onContactsSyncRequest(fromDevice: String, fromIp: String, fromPort: UShort) {
            Log.d("ConnectedApp", "onContactsSyncRequest from: $fromDevice ($fromIp:$fromPort)")
            // Cache the device address for sending notifications back
            trustedDeviceAddresses[fromDevice] = Pair(fromIp, fromPort)
            scope.launch {
                val deviceContacts = telephonyProvider.getContacts()
                Log.d("ConnectedApp", "Got ${deviceContacts.size} contacts to send")
                try {
                    sendContacts(fromIp, fromPort, deviceContacts)
                    Log.d("ConnectedApp", "Contacts sent to $fromDevice ($fromIp:$fromPort)")
                } catch (e: Exception) {
                    Log.e("ConnectedApp", "Failed to send contacts: ${e.message}")
                }
            }
        }

        override fun onContactsReceived(fromDevice: String, receivedContacts: List<FfiContact>) {
            runOnMainThread {
                contacts.clear()
                contacts.addAll(receivedContacts)
            }
        }

        override fun onConversationsSyncRequest(fromDevice: String, fromIp: String, fromPort: UShort) {
            Log.d("ConnectedApp", "onConversationsSyncRequest from: $fromDevice ($fromIp:$fromPort)")
            // Cache the device address for sending notifications back
            trustedDeviceAddresses[fromDevice] = Pair(fromIp, fromPort)
            scope.launch {
                val deviceConversations = telephonyProvider.getConversations()
                Log.d("ConnectedApp", "Got ${deviceConversations.size} conversations to send")
                try {
                    sendConversations(fromIp, fromPort, deviceConversations)
                    Log.d("ConnectedApp", "Conversations sent to $fromDevice ($fromIp:$fromPort)")
                } catch (e: Exception) {
                    Log.e("ConnectedApp", "Failed to send conversations: ${e.message}")
                }
            }
        }

        override fun onConversationsReceived(fromDevice: String, receivedConversations: List<FfiConversation>) {
            runOnMainThread {
                conversations.clear()
                conversations.addAll(receivedConversations)
            }
        }

        override fun onMessagesRequest(
            fromDevice: String,
            fromIp: String,
            fromPort: UShort,
            threadId: String,
            limit: UInt
        ) {
            Log.d("ConnectedApp", "onMessagesRequest from: $fromDevice ($fromIp:$fromPort), threadId: $threadId")
            // Cache the device address for sending notifications back
            trustedDeviceAddresses[fromDevice] = Pair(fromIp, fromPort)
            scope.launch {
                val messages = telephonyProvider.getMessages(threadId, limit.toInt())
                Log.d("ConnectedApp", "Got ${messages.size} messages to send")
                try {
                    sendMessages(fromIp, fromPort, threadId, messages)
                    Log.d("ConnectedApp", "Messages sent to $fromDevice ($fromIp:$fromPort)")
                } catch (e: Exception) {
                    Log.e("ConnectedApp", "Failed to send messages: ${e.message}")
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
            Log.d("ConnectedApp", "onSendSmsRequest from: $fromDevice ($fromIp:$fromPort), to: $to")
            // Cache the device address for sending notifications back
            trustedDeviceAddresses[fromDevice] = Pair(fromIp, fromPort)
            val result = telephonyProvider.sendSms(to, body)
            try {
                sendSmsSendResult(
                    fromIp,
                    fromPort,
                    result.isSuccess,
                    result.getOrNull(),
                    result.exceptionOrNull()?.message
                )
            } catch (e: Exception) {
                Log.e("ConnectedApp", "Failed to send SMS result: ${e.message}")
            }
        }

        override fun onSmsSendResult(success: Boolean, messageId: String?, error: String?) {
            runOnMainThread {
                if (success) {
                    transferStatus.value = "SMS sent successfully"
                } else {
                    transferStatus.value = "SMS failed: ${error ?: "Unknown error"}"
                }
            }
        }

        override fun onNewSms(fromDevice: String, message: FfiSmsMessage) {
            runOnMainThread {
                // Add to current messages if viewing the same thread
                if (currentMessages.isNotEmpty() && currentMessages.first().threadId == message.threadId) {
                    currentMessages.add(message)
                }
                // Update conversations
                scope.launch {
                    val device = devices.find { it.name == fromDevice }
                    device?.let { requestConversationsSync(it) }
                }
            }
        }

        override fun onCallLogRequest(fromDevice: String, fromIp: String, fromPort: UShort, limit: UInt) {
            Log.d("ConnectedApp", "onCallLogRequest from: $fromDevice ($fromIp:$fromPort), limit: $limit")
            // Cache the device address for sending notifications back
            trustedDeviceAddresses[fromDevice] = Pair(fromIp, fromPort)
            scope.launch {
                val entries = telephonyProvider.getCallLog(limit.toInt())
                Log.d("ConnectedApp", "Got ${entries.size} call log entries to send")
                try {
                    sendCallLog(fromIp, fromPort, entries)
                    Log.d("ConnectedApp", "Call log sent to $fromDevice ($fromIp:$fromPort)")
                } catch (e: Exception) {
                    Log.e("ConnectedApp", "Failed to send call log: ${e.message}")
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
            Log.d("ConnectedApp", "onInitiateCallRequest from: $fromDevice ($fromIp:$fromPort), number: $number")
            telephonyProvider.initiateCall(number)
        }

        override fun onCallActionRequest(fromDevice: String, fromIp: String, fromPort: UShort, action: CallAction) {
            Log.d("ConnectedApp", "onCallActionRequest from: $fromDevice ($fromIp:$fromPort), action: $action")
            telephonyProvider.performCallAction(action)
        }

        override fun onActiveCallUpdate(fromDevice: String, call: FfiActiveCall?) {
            runOnMainThread {
                activeCall.value = call
            }
        }
    }

    // Telephony listener for local events
    private val telephonyListener = object : TelephonyProvider.TelephonyListener {
        override fun onNewSmsReceived(message: FfiSmsMessage) {
            // Notify connected devices about new SMS
            Log.d("ConnectedApp", "onNewSmsReceived: ${message.address} - ${message.body.take(30)}")
            Log.d(
                "ConnectedApp",
                "Trusted devices: ${trustedDevices.size}, Discovered devices: ${devices.size}, Cached addresses: ${trustedDeviceAddresses.size}"
            )

            // First, try to notify using cached addresses (most reliable)
            trustedDeviceAddresses.forEach { (deviceName, address) ->
                val (ip, port) = address
                Log.d("ConnectedApp", "Sending new SMS notification to $deviceName ($ip:$port) via cached address")
                try {
                    uniffi.connected_ffi.notifyNewSms(ip, port.toUShort(), message)
                } catch (e: Exception) {
                    Log.e("ConnectedApp", "Failed to send SMS notification to $deviceName: ${e.message}")
                }
            }

            // Also try discovered devices that aren't in cache
            trustedDevices.forEach { deviceId ->
                val device = devices.find { it.id == deviceId }
                if (device != null && !trustedDeviceAddresses.containsKey(device.name)) {
                    Log.d(
                        "ConnectedApp",
                        "Sending new SMS notification to ${device.name} (${device.ip}:${device.port}) via discovery"
                    )
                    notifyNewSms(device, message)
                }
            }
        }

        override fun onCallStateChanged(call: FfiActiveCall?) {
            runOnMainThread {
                activeCall.value = call
            }
            // Notify connected devices about call state using cached addresses first
            trustedDeviceAddresses.forEach { (deviceName, address) ->
                val (ip, port) = address
                Log.d("ConnectedApp", "Sending call state update to $deviceName ($ip:$port) via cached address")
                try {
                    uniffi.connected_ffi.sendActiveCallUpdate(ip, port.toUShort(), call)
                } catch (e: Exception) {
                    Log.e("ConnectedApp", "Failed to send call state to $deviceName: ${e.message}")
                }
            }

            // Also try discovered devices that aren't in cache
            trustedDevices.forEach { deviceId ->
                val device = devices.find { it.id == deviceId }
                if (device != null && !trustedDeviceAddresses.containsKey(device.name)) {
                    sendActiveCallUpdate(device, call)
                }
            }
        }
    }

    private fun getPersistedRootUri(): Uri? {
        val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        val uriString = prefs.getString(PREF_ROOT_URI, null) ?: return null
        return Uri.parse(uriString)
    }

    fun setRootUri(uri: Uri) {
        // Persist permission ONLY for content URIs
        if (uri.scheme == "content") {
            val takeFlags: Int = android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION or
                    android.content.Intent.FLAG_GRANT_WRITE_URI_PERMISSION
            context.contentResolver.takePersistableUriPermission(uri, takeFlags)
        }

        val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        prefs.edit().putString(PREF_ROOT_URI, uri.toString()).apply()

        registerFsProvider(uri)
    }

    private fun registerFsProvider(uri: Uri) {
        try {
            val provider = AndroidFilesystemProvider(context, uri)
            uniffi.connected_ffi.registerFilesystemProvider(provider)
            isFsProviderRegistered.value = true

            // Update display name
            sharedFolderName.value = if (uri.scheme == "file") {
                "Full Device Access"
            } else {
                val doc = androidx.documentfile.provider.DocumentFile.fromTreeUri(context, uri)
                doc?.name ?: uri.path
            }

            Log.d("ConnectedApp", "Filesystem provider registered with URI: $uri")
        } catch (e: Exception) {
            Log.e("ConnectedApp", "Failed to register FS provider", e)
        }
    }

    fun setFullAccess() {
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.R) {
            val root = android.os.Environment.getExternalStorageDirectory()
            val uri = android.net.Uri.fromFile(root)
            setRootUri(uri)
        }
    }

    fun isFullAccessGranted(): Boolean {
        return if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.R) {
            android.os.Environment.isExternalStorageManager()
        } else {
            true
        }
    }

    fun requestFullAccessPermission() {
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.R) {
            try {
                val intent =
                    android.content.Intent(android.provider.Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION)
                intent.addCategory("android.intent.category.DEFAULT")
                intent.data = android.net.Uri.parse(String.format("package:%s", context.packageName))
                intent.flags = android.content.Intent.FLAG_ACTIVITY_NEW_TASK
                context.startActivity(intent)
            } catch (e: Exception) {
                Log.e("ConnectedApp", "Failed to launch permission settings", e)
            }
        }
    }

    fun browseRemoteFiles(device: DiscoveredDevice, path: String = "/") {
        browsingDevice = device
        isBrowsingRemote.value = true
        currentRemotePath.value = path

        // Clear thumbnails/requested for new directory if needed,
        // but here we keep them cached for session

        scope.launch(Dispatchers.IO) {
            try {
                val files = uniffi.connected_ffi.requestListDir(device.ip, device.port.toUShort(), path)
                withContext(Dispatchers.Main) {
                    remoteFiles.clear()
                    remoteFiles.addAll(files)
                }
            } catch (e: Exception) {
                Log.e("ConnectedApp", "Failed to browse remote files", e)
                withContext(Dispatchers.Main) {
                    android.widget.Toast.makeText(
                        context,
                        "Failed to list files: ${e.message}",
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
        thumbnails.clear()
        requestedThumbnails.clear()
    }

    fun getBrowsingDevice(): DiscoveredDevice? {
        return browsingDevice
    }

    fun getThumbnail(path: String) {
        if (thumbnails.containsKey(path) || requestedThumbnails.contains(path)) return
        val device = browsingDevice ?: return

        requestedThumbnails.add(path)
        scope.launch(Dispatchers.IO) {
            try {
                // Mark as requested/loading (optional, could use a separate set or placeholder)
                // For now just fetch
                val bytes = uniffi.connected_ffi.requestGetThumbnail(device.ip, device.port.toUShort(), path)
                if (bytes.isNotEmpty()) {
                    val bitmap = BitmapFactory.decodeByteArray(bytes, 0, bytes.size)
                    if (bitmap != null) {
                        runOnMainThread {
                            thumbnails[path] = bitmap
                        }
                    }
                }
            } catch (e: Exception) {
                Log.w("ConnectedApp", "Failed to get thumbnail for $path: ${e.message}")
            }
        }
    }

    private val discoveryCallback = object : DiscoveryCallback {
        override fun onDeviceFound(device: DiscoveredDevice) {
            Log.d("ConnectedApp", "Device found: ${device.name} (id=${device.id}, ip=${device.ip}:${device.port})")

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
            val isTrusted = isDeviceTrusted(device)
            Log.d("ConnectedApp", "Device ${device.name} (${device.id}) trust check result: $isTrusted")

            if (isTrusted) {
                if (!trustedDevices.contains(device.id)) {
                    trustedDevices.add(device.id)
                    Log.d("ConnectedApp", "Added ${device.name} to trustedDevices list")

                    // If it was pending, remove it
                    if (pendingPairing.contains(device.id)) {
                        pendingPairing.remove(device.id)
                    }

                    // Automatically pair (handshake) to confirm connection
                    // Only do this when we first discover/trust the device in this session
                    try {
                        Log.d("ConnectedApp", "Auto-connecting to trusted device ${device.name}")
                        uniffi.connected_ffi.pairDevice(device.ip, device.port)
                    } catch (e: Exception) {
                        Log.w("ConnectedApp", "Failed to auto-connect to trusted device", e)
                    }
                } else {
                    Log.d("ConnectedApp", "Device ${device.name} already in trustedDevices list")
                }
            } else {
                Log.d("ConnectedApp", "Device ${device.name} is NOT trusted - will show Pair button")
                // If device was in trustedDevices but is no longer trusted, remove it
                if (trustedDevices.contains(device.id)) {
                    trustedDevices.remove(device.id)
                    Log.d("ConnectedApp", "Removed ${device.name} from trustedDevices (no longer trusted in backend)")
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
            lastRemoteClipboard = text
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
            // Log result
            if (!success) {
                Log.e("ConnectedApp", "Clipboard send failed: $errorMsg")
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
            } catch (e: Exception) {
                Log.e("ConnectedApp", "Failed to copy to clipboard", e)
            }
        }
    }

    fun sendClipboard(device: DiscoveredDevice) {
        runOnMainThread {
            try {
                val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as android.content.ClipboardManager
                val item = clipboard.primaryClip?.getItemAt(0)
                val text = item?.text?.toString()

                if (!text.isNullOrEmpty()) {
                    uniffi.connected_ffi.sendClipboard(
                        device.ip,
                        device.port.toUShort(),
                        text,
                        clipboardCallback
                    )
                    android.widget.Toast.makeText(
                        context,
                        "Sending clipboard to ${device.name}",
                        android.widget.Toast.LENGTH_SHORT
                    ).show()
                } else {
                    android.widget.Toast.makeText(
                        context,
                        "Clipboard is empty",
                        android.widget.Toast.LENGTH_SHORT
                    ).show()
                }
            } catch (e: Exception) {
                Log.e("ConnectedApp", "Failed to send clipboard", e)
                android.widget.Toast.makeText(
                    context,
                    "Failed to access clipboard: ${e.message}",
                    android.widget.Toast.LENGTH_SHORT
                ).show()
            }
        }
    }


    // Media Control State
    val currentMediaTitle = mutableStateOf("Not Playing")
    val currentMediaArtist = mutableStateOf("")
    val currentMediaPlaying = mutableStateOf(false)
    private var lastMediaSourceDevice: String? = null // Fingerprint or Name
    private var lastBroadcastMediaState: MediaState? = null

    private fun initMediaSession() {
        if (mediaSession != null) return

        mediaSession = MediaSessionCompat(context, "ConnectedMediaSession").apply {
            setCallback(object : MediaSessionCompat.Callback() {
                override fun onPlay() {
                    sendMediaCommandToLastSource(MediaCommand.PLAY)
                }

                override fun onPause() {
                    sendMediaCommandToLastSource(MediaCommand.PAUSE)
                }

                override fun onSkipToNext() {
                    sendMediaCommandToLastSource(MediaCommand.NEXT)
                }

                override fun onSkipToPrevious() {
                    sendMediaCommandToLastSource(MediaCommand.PREVIOUS)
                }
            })
            isActive = true
        }
    }

    private fun sendMediaCommandToLastSource(command: MediaCommand) {
        // Find device by last source name or just pick the first trusted one if singular
        // Ideally we map `lastMediaSourceDevice` (name) to a DiscoveredDevice
        val targetName = lastMediaSourceDevice
        val device = devices.find { it.name == targetName && trustedDevices.contains(it.id) }
            ?: devices.firstOrNull { trustedDevices.contains(it.id) }

        if (device != null) {
            sendMediaCommand(device, command)
        } else {
            Log.w("ConnectedApp", "No trusted device found to send media command")
        }
    }

    private fun updateMediaSession(state: MediaState) {
        val session = mediaSession ?: return

        val metadataBuilder = MediaMetadataCompat.Builder()
        metadataBuilder.putString(MediaMetadataCompat.METADATA_KEY_TITLE, state.title ?: "Unknown Title")
        metadataBuilder.putString(MediaMetadataCompat.METADATA_KEY_ARTIST, state.artist ?: "Unknown Artist")
        metadataBuilder.putString(MediaMetadataCompat.METADATA_KEY_ALBUM, state.album ?: "")

        session.setMetadata(metadataBuilder.build())

        val playbackStateBuilder = PlaybackStateCompat.Builder()
        val actions = PlaybackStateCompat.ACTION_PLAY or
                PlaybackStateCompat.ACTION_PAUSE or
                PlaybackStateCompat.ACTION_PLAY_PAUSE or
                PlaybackStateCompat.ACTION_SKIP_TO_NEXT or
                PlaybackStateCompat.ACTION_SKIP_TO_PREVIOUS

        val stateCode = if (state.playing) PlaybackStateCompat.STATE_PLAYING else PlaybackStateCompat.STATE_PAUSED

        playbackStateBuilder.setActions(actions)
        playbackStateBuilder.setState(stateCode, PlaybackStateCompat.PLAYBACK_POSITION_UNKNOWN, 1.0f)

        session.setPlaybackState(playbackStateBuilder.build())
        session.isActive = true // Ensure session is active when we have state
    }

    fun onLocalMediaUpdate(state: MediaState) {
        if (!isMediaControlEnabled.value) return

        // Only broadcast if changed to avoid spam (deduplicate against LAST SENT state, not remote state)
        if (state == lastBroadcastMediaState) return

        Log.d("ConnectedApp", "Broadcasting local media update: ${state.title}")
        lastBroadcastMediaState = state

        // Broadcast to all trusted devices
        for (deviceId in trustedDevices) {
            val device = devices.find { it.id == deviceId }
            if (device != null) {
                try {
                    uniffi.connected_ffi.sendMediaState(device.ip, device.port.toUShort(), state)
                } catch (e: Exception) {
                    Log.e("ConnectedApp", "Failed to send media state to ${device.name}", e)
                }
            }
        }
    }

    private val mediaCallback = object : MediaControlCallback {
        override fun onMediaCommand(fromDevice: String, command: MediaCommand) {
            // Android media control implementation would go here (using MediaSession)
            // For now, we just log it as we primarily focus on Desktop acting as the receiver/player
            Log.d("ConnectedApp", "Received media command from $fromDevice: $command")

            runOnMainThread {
                // Temporarily deactivate our session so we don't swallow the key event
                val wasActive = mediaSession?.isActive == true
                if (wasActive) {
                    mediaSession?.isActive = false
                }

                // Execute command on Android using AudioManager
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

                    android.widget.Toast.makeText(context, "Executed: $command", android.widget.Toast.LENGTH_SHORT)
                        .show()
                }

                // Restore active state
                if (wasActive) {
                    mediaSession?.isActive = true
                }
            }
        }

        override fun onMediaStateUpdate(fromDevice: String, state: MediaState) {
            Log.d("ConnectedApp", "Media update from $fromDevice: ${state.title}")
            // Update local state for UI display
            lastMediaSourceDevice = fromDevice
            currentMediaTitle.value = state.title ?: "Unknown Title"
            currentMediaArtist.value = state.artist ?: "Unknown Artist"
            currentMediaPlaying.value = state.playing

            // Update Android MediaSession for notification controls
            if (isMediaControlEnabled.value) {
                runOnMainThread {
                    initMediaSession()
                    updateMediaSession(state)
                }
            }
        }
    }

    fun toggleMediaControl() {
        isMediaControlEnabled.value = !isMediaControlEnabled.value

        // Persist state
        context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
            .edit()
            .putBoolean(PREF_MEDIA_CONTROL, isMediaControlEnabled.value)
            .apply()

        val msg = if (isMediaControlEnabled.value) "Media Control Enabled" else "Media Control Disabled"
        android.widget.Toast.makeText(context, msg, android.widget.Toast.LENGTH_SHORT).show()

        if (!isMediaControlEnabled.value) {
            mediaSession?.isActive = false
            mediaSession?.release()
            mediaSession = null
        }
    }

    fun sendMediaCommand(device: DiscoveredDevice, command: MediaCommand) {
        try {
            uniffi.connected_ffi.sendMediaCommand(device.ip, device.port.toUShort(), command)
        } catch (e: Exception) {
            Log.e("ConnectedApp", "Failed to send media command", e)
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

    fun stopSharing() {
        val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        prefs.edit().remove(PREF_ROOT_URI).apply()

        sharedFolderName.value = null
        isFsProviderRegistered.value = false

        android.widget.Toast.makeText(context, "File sharing stopped", android.widget.Toast.LENGTH_SHORT).show()
    }

    fun toggleTelephony() {
        val enabled = !isTelephonyEnabled.value
        isTelephonyEnabled.value = enabled

        // Persist state
        context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
            .edit()
            .putBoolean(PREF_TELEPHONY_ENABLED, enabled)
            .apply()

        if (enabled) {
            telephonyProvider.setListener(telephonyListener)
            telephonyProvider.registerReceivers()
            registerTelephonyCallback(telephonyCallback)
        } else {
            telephonyProvider.setListener(null)
            telephonyProvider.unregisterReceivers()
        }
    }

    // Telephony send functions
    fun requestContactsSync(device: DiscoveredDevice) {
        try {
            requestContactsSync(device.ip, device.port)
        } catch (e: Exception) {
            e.printStackTrace()
        }
    }

    fun sendContacts(device: DiscoveredDevice, contactsList: List<FfiContact>) {
        try {
            sendContacts(device.ip, device.port, contactsList)
        } catch (e: Exception) {
            e.printStackTrace()
        }
    }

    fun sendContacts(ip: String, port: UShort, contactsList: List<FfiContact>) {
        uniffi.connected_ffi.sendContacts(ip, port.toUShort(), contactsList)
    }

    fun requestConversationsSync(device: DiscoveredDevice) {
        try {
            requestConversationsSync(device.ip, device.port)
        } catch (e: Exception) {
            e.printStackTrace()
        }
    }

    fun sendConversations(device: DiscoveredDevice, convos: List<FfiConversation>) {
        try {
            uniffi.connected_ffi.sendConversations(device.ip, device.port, convos)
        } catch (e: Exception) {
            e.printStackTrace()
        }
    }

    fun sendConversations(ip: String, port: UShort, convos: List<FfiConversation>) {
        uniffi.connected_ffi.sendConversations(ip, port.toUShort(), convos)
    }

    fun requestMessages(device: DiscoveredDevice, threadId: String, limit: Int = 50) {
        try {
            requestMessages(device.ip, device.port, threadId, limit.toUInt())
        } catch (e: Exception) {
            e.printStackTrace()
        }
    }

    fun sendMessages(device: DiscoveredDevice, threadId: String, messages: List<FfiSmsMessage>) {
        try {
            uniffi.connected_ffi.sendMessages(device.ip, device.port, threadId, messages)
        } catch (e: Exception) {
            e.printStackTrace()
        }
    }

    fun sendMessages(ip: String, port: UShort, threadId: String, messages: List<FfiSmsMessage>) {
        uniffi.connected_ffi.sendMessages(ip, port.toUShort(), threadId, messages)
    }

    fun sendSmsToDevice(device: DiscoveredDevice, to: String, body: String) {
        try {
            sendSms(device.ip, device.port, to, body)
        } catch (e: Exception) {
            e.printStackTrace()
        }
    }

    fun sendSmsSendResult(
        device: DiscoveredDevice,
        success: Boolean,
        messageId: String?,
        error: String?
    ) {
        sendSmsSendResult(device.ip, device.port, success, messageId, error)
    }

    fun sendSmsSendResult(
        ip: String,
        port: UShort,
        success: Boolean,
        messageId: String?,
        error: String?
    ) {
        try {
            uniffi.connected_ffi.sendSmsSendResult(ip, port.toUShort(), success, messageId, error)
        } catch (e: Exception) {
            android.util.Log.e("ConnectedApp", "Failed to send SMS result: ${e.message}")
        }
    }

    fun notifyNewSms(device: DiscoveredDevice, message: FfiSmsMessage) {
        try {
            notifyNewSms(device.ip, device.port, message)
        } catch (e: Exception) {
            e.printStackTrace()
        }
    }

    fun requestCallLog(device: DiscoveredDevice, limit: Int = 50) {
        try {
            requestCallLog(device.ip, device.port, limit.toUInt())
        } catch (e: Exception) {
            e.printStackTrace()
        }
    }

    fun sendCallLog(device: DiscoveredDevice, entries: List<FfiCallLogEntry>) {
        try {
            uniffi.connected_ffi.sendCallLog(device.ip, device.port, entries)
        } catch (e: Exception) {
            e.printStackTrace()
        }
    }

    fun sendCallLog(ip: String, port: UShort, entries: List<FfiCallLogEntry>) {
        uniffi.connected_ffi.sendCallLog(ip, port.toUShort(), entries)
    }

    fun initiateCallOnDevice(device: DiscoveredDevice, number: String) {
        try {
            initiateCall(device.ip, device.port, number)
        } catch (e: Exception) {
            e.printStackTrace()
        }
    }

    fun sendCallActionToDevice(device: DiscoveredDevice, action: CallAction) {
        try {
            sendCallAction(device.ip, device.port, action)
        } catch (e: Exception) {
            e.printStackTrace()
        }
    }

    fun sendActiveCallUpdate(device: DiscoveredDevice, call: FfiActiveCall?) {
        try {
            sendActiveCallUpdate(device.ip, device.port, call)
        } catch (e: Exception) {
            e.printStackTrace()
        }
    }

    fun initialize() {
        try {
            // Acquire MulticastLock for mDNS
            val wifiManager =
                context.applicationContext.getSystemService(Context.WIFI_SERVICE) as android.net.wifi.WifiManager
            multicastLock = wifiManager.createMulticastLock("ConnectedMulticastLock")
            multicastLock?.setReferenceCounted(true)
            multicastLock?.acquire()
            Log.d("ConnectedApp", "Multicast lock acquired")

            // Create a dedicated download directory in the app's private storage
            // The core will append "downloads" to the storage path we provide
            downloadDir = File(context.getExternalFilesDir(null), "downloads")
            if (!downloadDir.exists()) {
                downloadDir.mkdirs()
            }

            // Pass the root files directory. Core will join("downloads") to this.
            val storagePath = context.getExternalFilesDir(null)?.absolutePath ?: ""

            uniffi.connected_ffi.initialize(
                getDeviceName(),
                "Mobile",
                0u.toUShort(),
                storagePath
            )
            uniffi.connected_ffi.startDiscovery(discoveryCallback)
            uniffi.connected_ffi.registerTransferCallback(transferCallback)
            uniffi.connected_ffi.registerClipboardReceiver(clipboardCallback)
            uniffi.connected_ffi.registerPairingCallback(pairingCallback)
            uniffi.connected_ffi.registerUnpairCallback(unpairCallback)
            uniffi.connected_ffi.registerMediaControlCallback(mediaCallback)

            // Auto-register filesystem if permission exists
            getPersistedRootUri()?.let { uri ->
                registerFsProvider(uri)
            }

            // Load persisted settings
            val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)

            // Restore Clipboard Sync
            if (prefs.getBoolean(PREF_CLIPBOARD_SYNC, false)) {
                startClipboardSync()
            }

            // Restore Media Control
            isMediaControlEnabled.value = prefs.getBoolean(PREF_MEDIA_CONTROL, false)

            // Restore Telephony
            if (prefs.getBoolean(PREF_TELEPHONY_ENABLED, false)) {
                isTelephonyEnabled.value = true
                telephonyProvider.setListener(telephonyListener)
                telephonyProvider.registerReceivers()
                // Use the fully qualified name or the imported function if available
                // Assuming registerTelephonyCallback is available from uniffi.connected_ffi.*
                uniffi.connected_ffi.registerTelephonyCallback(telephonyCallback)
            }

        } catch (e: Exception) {
            Log.e("ConnectedApp", "Initialization failed", e)
        }
    }

    fun downloadRemoteFile(device: DiscoveredDevice, remotePath: String) {
        scope.launch(Dispatchers.IO) {
            try {
                withContext(Dispatchers.Main) {
                    android.widget.Toast.makeText(
                        context,
                        "Downloading...",
                        android.widget.Toast.LENGTH_SHORT
                    ).show()
                }

                val fileName = remotePath.substringAfterLast('/')
                val downloadFile = File(downloadDir, fileName)

                uniffi.connected_ffi.requestDownloadFile(
                    device.ip,
                    device.port.toUShort(),
                    remotePath,
                    downloadFile.absolutePath
                )

                val uri = moveToDownloads(fileName)

                withContext(Dispatchers.Main) {
                    // Try to open if possible
                    if (uri != null) {
                        openFile(uri)
                    }
                }
            } catch (e: Exception) {
                Log.e("ConnectedApp", "Download failed", e)
                withContext(Dispatchers.Main) {
                    android.widget.Toast.makeText(
                        context,
                        "Download failed: ${e.message}",
                        android.widget.Toast.LENGTH_LONG
                    ).show()
                }
            }
        }
    }

    private fun openFile(uri: Uri) {
        try {
            val intent = android.content.Intent(android.content.Intent.ACTION_VIEW)
            intent.setDataAndType(uri, getMimeType(uri.toString()))
            intent.addFlags(android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION)
            intent.addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK)
            context.startActivity(intent)
        } catch (e: Exception) {
            Log.e("ConnectedApp", "Failed to open file", e)
            android.widget.Toast.makeText(
                context,
                "Cannot open file",
                android.widget.Toast.LENGTH_SHORT
            ).show()
        }
    }

    private fun moveToDownloads(filename: String): Uri? {
        val sourceFile = File(downloadDir, filename)
        if (!sourceFile.exists()) {
            Log.e("ConnectedApp", "Source file not found: ${sourceFile.absolutePath}")
            return null
        }

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
                return itemUri
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
        return null
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
            android.widget.Toast.makeText(
                context,
                "Pairing failed: ${e.message}",
                android.widget.Toast.LENGTH_SHORT
            )
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
            android.widget.Toast.makeText(
                context,
                "Forget failed: ${e.message}",
                android.widget.Toast.LENGTH_SHORT
            )
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
            android.widget.Toast.makeText(
                context,
                "Block failed: ${e.message}",
                android.widget.Toast.LENGTH_SHORT
            )
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
            android.widget.Toast.makeText(
                context,
                "Blocked ${request.deviceName}",
                android.widget.Toast.LENGTH_SHORT
            )
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

    fun sendFolderToDevice(device: DiscoveredDevice, treeUri: Uri) {
        scope.launch(Dispatchers.IO) {
            try {
                val documentFile = androidx.documentfile.provider.DocumentFile.fromTreeUri(context, treeUri)
                if (documentFile == null) {
                    withContext(Dispatchers.Main) {
                        android.widget.Toast.makeText(
                            context,
                            "Invalid folder selection",
                            android.widget.Toast.LENGTH_SHORT
                        ).show()
                    }
                    return@launch
                }

                val folderName = documentFile.name ?: "Folder"
                withContext(Dispatchers.Main) {
                    android.widget.Toast.makeText(
                        context,
                        "Preparing folder: $folderName...",
                        android.widget.Toast.LENGTH_SHORT
                    ).show()
                }

                // Create a temporary directory structure
                val tempRoot = File(context.cacheDir, "folder_transfers")
                if (tempRoot.exists()) {
                    tempRoot.deleteRecursively()
                }
                tempRoot.mkdirs()

                val destFolder = File(tempRoot, folderName)

                if (copyDocumentFileToLocal(documentFile, destFolder)) {
                    withContext(Dispatchers.Main) {
                        android.widget.Toast.makeText(
                            context,
                            "Sending folder: $folderName",
                            android.widget.Toast.LENGTH_SHORT
                        ).show()
                    }
                    uniffi.connected_ffi.sendFile(device.ip, device.port.toUShort(), destFolder.absolutePath)
                } else {
                    withContext(Dispatchers.Main) {
                        android.widget.Toast.makeText(
                            context,
                            "Failed to prepare folder",
                            android.widget.Toast.LENGTH_SHORT
                        ).show()
                    }
                }

            } catch (e: Exception) {
                Log.e("ConnectedApp", "Send folder failed", e)
                withContext(Dispatchers.Main) {
                    android.widget.Toast.makeText(
                        context,
                        "Failed to send folder: ${e.message}",
                        android.widget.Toast.LENGTH_LONG
                    ).show()
                }
            }
        }
    }

    private fun copyDocumentFileToLocal(source: androidx.documentfile.provider.DocumentFile, dest: File): Boolean {
        if (source.isDirectory) {
            if (!dest.exists() && !dest.mkdirs()) return false

            val files = source.listFiles()
            for (child in files) {
                val childDest = File(dest, child.name ?: "unknown")
                if (!copyDocumentFileToLocal(child, childDest)) return false
            }
            return true
        } else {
            // Copy file content
            return try {
                context.contentResolver.openInputStream(source.uri)?.use { input ->
                    dest.outputStream().use { output ->
                        input.copyTo(output)
                    }
                }
                true
            } catch (e: Exception) {
                Log.e("ConnectedApp", "Failed to copy file ${source.name}", e)
                false
            }
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
                val nameIndex =
                    it.getColumnIndex(android.provider.MediaStore.Files.FileColumns.DISPLAY_NAME)
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
        stopClipboardSync()
        uniffi.connected_ffi.shutdown()

        try {
            multicastLock?.release()
            Log.d("ConnectedApp", "Multicast lock released")
        } catch (e: Exception) {
            Log.e("ConnectedApp", "Error releasing multicast lock", e)
        }
    }

    fun toggleClipboardSync() {
        if (isClipboardSyncEnabled.value) {
            stopClipboardSync()
        } else {
            startClipboardSync()
        }

        // Persist state
        context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
            .edit()
            .putBoolean(PREF_CLIPBOARD_SYNC, isClipboardSyncEnabled.value)
            .apply()
    }

    private fun startClipboardSync() {
        if (isClipboardSyncEnabled.value) return
        isClipboardSyncEnabled.value = true

        runOnMainThread {
            try {
                val clipboard =
                    context.getSystemService(Context.CLIPBOARD_SERVICE) as android.content.ClipboardManager
                clipboard.primaryClip?.getItemAt(0)?.text?.toString()?.let {
                    lastLocalClipboard = it
                }
            } catch (e: Exception) {
                Log.e("ConnectedApp", "Failed to init clipboard sync", e)
            }
        }

        clipboardSyncJob = scope.launch {
            while (isActive && isClipboardSyncEnabled.value) {
                try {
                    val currentClipboard = withContext(Dispatchers.Main) {
                        try {
                            val clipboard =
                                context.getSystemService(Context.CLIPBOARD_SERVICE) as android.content.ClipboardManager
                            clipboard.primaryClip?.getItemAt(0)?.text?.toString() ?: ""
                        } catch (e: Exception) {
                            Log.e("ConnectedApp", "Failed to read clipboard", e)
                            ""
                        }
                    }

                    if (currentClipboard.isNotEmpty() &&
                        currentClipboard != lastLocalClipboard &&
                        currentClipboard != lastRemoteClipboard
                    ) {
                        Log.d("ConnectedApp", "Clipboard changed, broadcasting")
                        lastLocalClipboard = currentClipboard

                        // Broadcast to all trusted devices
                        for (deviceId in trustedDevices) {
                            val device = devices.find { it.id == deviceId }
                            if (device != null) {
                                try {
                                    uniffi.connected_ffi.sendClipboard(
                                        device.ip,
                                        device.port.toUShort(),
                                        currentClipboard,
                                        clipboardCallback
                                    )
                                } catch (e: Exception) {
                                    Log.e(
                                        "ConnectedApp",
                                        "Failed to sync clipboard to ${device.name}",
                                        e
                                    )
                                }
                            }
                        }
                    }
                } catch (e: Exception) {
                    Log.e("ConnectedApp", "Clipboard sync loop error", e)
                }
                delay(1000)
            }
        }

        android.widget.Toast.makeText(
            context,
            "Clipboard Sync Started",
            android.widget.Toast.LENGTH_SHORT
        ).show()
    }

    private fun stopClipboardSync() {
        isClipboardSyncEnabled.value = false
        clipboardSyncJob?.cancel()
        clipboardSyncJob = null
        lastLocalClipboard = ""
        lastRemoteClipboard = ""
        android.widget.Toast.makeText(
            context,
            "Clipboard Sync Stopped",
            android.widget.Toast.LENGTH_SHORT
        ).show()
    }
}
