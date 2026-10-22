package com.connected.app.sync

import android.Manifest
import android.annotation.SuppressLint
import java.util.Locale
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.os.Build
import android.os.SystemClock
import android.content.pm.PackageManager
import android.provider.Settings
import android.text.TextUtils
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Warning
import androidx.compose.material.icons.filled.Close
import androidx.compose.material3.TextButton
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.nestedscroll.NestedScrollConnection
import androidx.compose.ui.input.nestedscroll.NestedScrollSource
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.Velocity
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.core.content.ContextCompat
import androidx.core.os.BundleCompat
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlin.math.abs
import uniffi.connected_ffi.DiscoveredDevice
import uniffi.connected_ffi.DeviceType
import uniffi.connected_ffi.RemoteCommand
import uniffi.connected_ffi.RemoteInputEvent
import uniffi.connected_ffi.FfiCallLogEntry
import uniffi.connected_ffi.FfiContact
import uniffi.connected_ffi.FfiConversation
import uniffi.connected_ffi.FfiSmsMessage
import uniffi.connected_ffi.getTrustedPeers
import androidx.core.net.toUri

fun DeviceType.displayName() = when (this) {
    DeviceType.ANDROID -> "Android"
    DeviceType.IOS -> "iOS"
    DeviceType.LINUX -> "Linux"
    DeviceType.WINDOWS -> "Windows"
    DeviceType.MAC_OS -> "macOS"
    DeviceType.UNKNOWN -> "Unknown"
}

@OptIn(ExperimentalMaterial3Api::class)
class MainActivity : ComponentActivity() {
    private lateinit var connectedApp: ConnectedApp
    private lateinit var proximityPermissionsLauncher: ActivityResultLauncher<Array<String>>
    private var proximityPermissionsInFlight = false

    private val filePickerLauncher = registerForActivityResult(ActivityResultContracts.GetMultipleContents()) { uris ->
        if (uris.isNotEmpty()) {
            connectedApp.getSelectedDeviceForFileTransfer()?.let { device ->
                lifecycleScope.launch {
                    uris.forEachIndexed { index, selectedUri ->
                        if (index > 0) delay(200) // Small delay to avoid overwhelming the connection
                        connectedApp.sendFileToDevice(device, selectedUri.toString())
                    }
                }
            }
        }
    }

    private val folderPickerLauncher = registerForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri ->
        uri?.let {
            connectedApp.setRootUri(it)
        }
    }

    private val downloadDirPickerLauncher =
        registerForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri ->
            uri?.let {
                connectedApp.setCustomDownloadDir(it)
            }
        }

    private val sendFolderLauncher = registerForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri ->
        uri?.let { selectedUri ->
            connectedApp.getSelectedDeviceForFileTransfer()?.let { device ->
                connectedApp.sendFolderToDevice(device, selectedUri)
            }
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)

        // Initialize singleton with Application Context
        connectedApp = ConnectedApp.getInstance(applicationContext)
        handleNotificationIntent(intent)

        // If service is running, it has already initialized the app logic.
        if (!ConnectedService.isRunning) {
            connectedApp.initialize()
        }

        proximityPermissionsLauncher = registerForActivityResult(
            ActivityResultContracts.RequestMultiplePermissions()
        ) {
            proximityPermissionsInFlight = false
            connectedApp.startWifiAwareManager()
        }

        requestProximityPermissionsIfNeeded()
        handleShareIntent(intent)

        setContent {
            ConnectedTheme(themeMode = connectedApp.themeMode.value) {
                Surface(modifier = Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
                    if (connectedApp.isBrowsingRemote.value) {
                        RemoteFileBrowser(connectedApp)
                    } else {
                        MainAppNavigation(
                            connectedApp,
                            filePickerLauncher,
                            folderPickerLauncher,
                            sendFolderLauncher,
                            downloadDirPickerLauncher
                        )
                    }
                }
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handleNotificationIntent(intent)
        handleShareIntent(intent)
    }

    override fun onDestroy() {
        // Only tear down when the activity is truly finishing AND we are not
        // running as a service. Configuration changes (rotation, theme change,
        // split-screen resize) also destroy the activity — shutting down
        // discovery/transfers/the Rust core for those was wrong, and the
        // synchronous cleanup blocked the main thread mid-rotation.
        if (isFinishing && !ConnectedService.isRunning) {
            connectedApp.cleanupAsync()
        }
        super.onDestroy()
    }

    override fun onResume() {
        super.onResume()
        connectedApp.setAppInForeground(true)
        requestProximityPermissionsIfNeeded()
    }

    override fun onPause() {
        connectedApp.setAppInForeground(false)
        super.onPause()
    }

    private fun requestProximityPermissionsIfNeeded() {
        val missing = LinkedHashSet<String>()

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            if (ContextCompat.checkSelfPermission(this, Manifest.permission.NEARBY_WIFI_DEVICES) !=
                PackageManager.PERMISSION_GRANTED
            ) {
                missing.add(Manifest.permission.NEARBY_WIFI_DEVICES)
            }
        } else {
            if (ContextCompat.checkSelfPermission(this, Manifest.permission.ACCESS_FINE_LOCATION) !=
                PackageManager.PERMISSION_GRANTED
            ) {
                missing.add(Manifest.permission.ACCESS_FINE_LOCATION)
            }
            if (ContextCompat.checkSelfPermission(this, Manifest.permission.ACCESS_COARSE_LOCATION) !=
                PackageManager.PERMISSION_GRANTED
            ) {
                missing.add(Manifest.permission.ACCESS_COARSE_LOCATION)
            }
        }

        if (missing.isEmpty()) {
            connectedApp.startWifiAwareManager()
            return
        }

        if (!proximityPermissionsInFlight) {
            proximityPermissionsInFlight = true
            proximityPermissionsLauncher.launch(missing.toTypedArray())
        }
    }

    private fun handleShareIntent(intent: Intent?) {
        if (intent == null) return
        when (intent.action) {
            Intent.ACTION_SEND,
            Intent.ACTION_SEND_MULTIPLE -> {
                val sharedUrl = extractSharedUrl(intent)
                if (sharedUrl != null) {
                    connectedApp.setPendingShareUrl(sharedUrl)
                } else {
                    val uris = collectShareUris(intent)
                    if (uris.isNotEmpty()) {
                        connectedApp.setPendingShare(uris)
                    }
                }
            }
        }
    }

    private fun extractSharedUrl(intent: Intent): String? {
        val text = intent.getStringExtra(Intent.EXTRA_TEXT)
            ?: intent.clipData?.let { clip ->
                if (clip.itemCount > 0) clip.getItemAt(0)?.text?.toString() else null
            }
        if (!text.isNullOrBlank()) {
            val regex = Regex("""https?://[^\s]+""")
            val match = regex.find(text)
            if (match != null) {
                return match.value
            }
        }
        return null
    }

    private fun collectShareUris(intent: Intent): List<Uri> {
        val result = linkedSetOf<Uri>()

        intent.extras?.let { extras ->
            BundleCompat.getParcelable(extras, Intent.EXTRA_STREAM, Uri::class.java)?.let { result.add(it) }
            BundleCompat.getParcelableArrayList(extras, Intent.EXTRA_STREAM, Uri::class.java)
                ?.forEach { result.add(it) }

            BundleCompat.getParcelableArray(extras, Intent.EXTRA_STREAM, Uri::class.java)
                ?.forEach { (it as? Uri)?.let { uri -> result.add(uri) } }
        }

        intent.clipData?.let { clip ->
            for (index in 0 until clip.itemCount) {
                clip.getItemAt(index)?.uri?.let { result.add(it) }
            }
        }

        if (intent.action == Intent.ACTION_SEND && result.isEmpty()) {
            intent.data?.let { result.add(it) }
        }

        return result.toList()
    }

    private fun handleNotificationIntent(intent: Intent?) {
        if (intent == null) return
        val handled = when (intent.action) {
            ConnectedApp.ACTION_OPEN_TRANSFER_REQUEST -> {
                connectedApp.openTransferRequestFromIntent(intent)
                true
            }

            ConnectedApp.ACTION_OPEN_PAIRING_REQUEST -> {
                connectedApp.openPairingRequestFromIntent(intent)
                true
            }

            ConnectedApp.ACTION_OPEN_COMPLETED_FILE -> {
                connectedApp.openCompletedFileFromIntent(intent)
                true
            }

            ConnectedApp.ACTION_RESUME_BACKGROUND_SERVICE -> {
                // The user tapped the "Background sync stopped" notification.
                // The app is in the foreground now, so starting the foreground
                // service is always allowed.
                startForegroundService(Intent(this, ConnectedService::class.java))
                true
            }

            else -> false
        }

        if (handled) {
            intent.action = null
        }
    }
}

/**
 * Robustly checks if the NotificationListenerService is enabled.
 * Handles variations in how different Android versions and OEMs format the enabled_notification_listeners string.
 */
fun isNotificationListenerEnabled(context: Context, serviceClass: Class<*>): Boolean {
    val componentName = android.content.ComponentName(context, serviceClass)
    val flat = Settings.Secure.getString(context.contentResolver, "enabled_notification_listeners")

    if (TextUtils.isEmpty(flat)) {
        return false
    }

    val flattenedName = componentName.flattenToString()
    val flattenedShortName = componentName.flattenToShortString()

    // Check both formats as different Android versions/OEMs may use different formats
    return flat.split(":").any { enabledComponent ->
        val trimmed = enabledComponent.trim()
        trimmed == flattenedName ||
                trimmed == flattenedShortName ||
                trimmed.contains(serviceClass.name) ||
                // Some OEMs use just the class name without package
                trimmed.endsWith("/" + serviceClass.simpleName)
    }
}

/**
 * Opens the notification listener settings screen.
 * Uses the proper Settings constant and handles potential exceptions.
 */
fun openNotificationListenerSettings(context: Context) {
    try {
        val intent = Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS)
        intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        context.startActivity(intent)
    } catch (_: Exception) {
        // Fallback for devices that don't support the direct intent
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                val fallbackIntent = Intent(Settings.ACTION_NOTIFICATION_LISTENER_DETAIL_SETTINGS)
                fallbackIntent.putExtra(
                    Settings.EXTRA_NOTIFICATION_LISTENER_COMPONENT_NAME,
                    android.content.ComponentName(context, MediaObserverService::class.java).flattenToString()
                )
                fallbackIntent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                context.startActivity(fallbackIntent)
            } else {
                throw Exception("Direct settings not available")
            }
        } catch (_: Exception) {
            // Final fallback to general notification settings
            val generalIntent = Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS)
            generalIntent.putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName)
            generalIntent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            context.startActivity(generalIntent)
        }
    }
}

@Composable
fun RemoteFileBrowser(app: ConnectedApp) {
    val downloadProgress = app.browserDownloadProgress.value

    Column(modifier = Modifier.fillMaxSize().safeDrawingPadding().padding(16.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = { app.closeRemoteBrowser() }) {
                Icon(painterResource(R.drawable.ic_back), contentDescription = "Back")
            }
            Text("Remote Files: ${app.currentRemotePath.value}", style = MaterialTheme.typography.titleMedium)
        }

        // Download progress indicator
        if (downloadProgress != null) {
            Card(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(vertical = 8.dp),
                colors = CardDefaults.cardColors(
                    containerColor = MaterialTheme.colorScheme.primaryContainer
                )
            ) {
                Column(modifier = Modifier.padding(16.dp)) {
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.SpaceBetween,
                        verticalAlignment = Alignment.CenterVertically
                    ) {
                        Text(
                            text = if (downloadProgress.isFolder) "Downloading folder..." else "Downloading...",
                            style = MaterialTheme.typography.titleSmall
                        )
                        Text(
                            text = "${downloadProgress.percentComplete.toInt()}%",
                            style = MaterialTheme.typography.bodyMedium
                        )
                    }
                    Spacer(modifier = Modifier.height(4.dp))
                    Text(
                        text = downloadProgress.currentFile,
                        style = MaterialTheme.typography.bodySmall,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        color = MaterialTheme.colorScheme.onPrimaryContainer.copy(alpha = 0.7f)
                    )
                    Spacer(modifier = Modifier.height(8.dp))
                    LinearProgressIndicator(
                        progress = { downloadProgress.percentComplete / 100f },
                        modifier = Modifier.fillMaxWidth(),
                    )
                    Spacer(modifier = Modifier.height(4.dp))
                    Text(
                        text = "${formatFileSize(downloadProgress.bytesDownloaded.toULong())} / ${
                            formatFileSize(
                                downloadProgress.totalBytes.toULong()
                            )
                        }",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onPrimaryContainer.copy(alpha = 0.7f)
                    )
                }
            }
        }

        LazyColumn(modifier = Modifier.padding(top = 8.dp)) {
            if (app.currentRemotePath.value != "/") {
                item {
                    Card(
                        modifier = Modifier.padding(vertical = 4.dp).fillMaxWidth(),
                        onClick = {
                            val current = app.currentRemotePath.value
                            val parent = current.substringBeforeLast('/').ifEmpty { "/" }
                            app.getBrowsingDevice()?.let { device ->
                                app.browseRemoteFiles(device, parent)
                            }
                        }
                    ) {
                        Row(modifier = Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                            Icon(
                                painterResource(R.drawable.ic_folder),
                                contentDescription = "Folder",
                                tint = MaterialTheme.colorScheme.primary
                            )
                            Spacer(modifier = Modifier.width(8.dp))
                            Text("..")
                        }
                    }
                }
            }

            items(app.remoteFiles) { file ->
                // Check if it's an image
                val ext = file.name.substringAfterLast('.', "").lowercase()
                val isImage = ext in listOf("jpg", "jpeg", "png", "gif", "webp", "bmp")

                // Request thumbnail if needed
                if (isImage && file.entryType == uniffi.connected_ffi.FfiFsEntryType.FILE) {
                    app.getThumbnail(file.path)
                }

                val isDirectory = file.entryType == uniffi.connected_ffi.FfiFsEntryType.DIRECTORY
                Card(
                    modifier = Modifier.padding(vertical = 4.dp).fillMaxWidth(),
                    onClick = {
                        if (isDirectory) {
                            app.getBrowsingDevice()?.let { device ->
                                app.browseRemoteFiles(device, file.path)
                            }
                        }
                    }
                ) {
                    Row(
                        modifier = Modifier.padding(16.dp).fillMaxWidth(),
                        horizontalArrangement = Arrangement.SpaceBetween,
                        verticalAlignment = Alignment.CenterVertically
                    ) {
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                            modifier = Modifier.weight(1f)
                        ) {
                            if (isImage && app.thumbnails.containsKey(file.path)) {
                                val bitmap = app.thumbnails[file.path]!!
                                Image(
                                    bitmap = bitmap.asImageBitmap(),
                                    contentDescription = null,
                                    contentScale = ContentScale.Crop,
                                    modifier = Modifier
                                        .size(24.dp)
                                        .padding(end = 8.dp)
                                )
                            } else {
                                val iconRes =
                                    if (file.entryType == uniffi.connected_ffi.FfiFsEntryType.DIRECTORY) R.drawable.ic_folder else R.drawable.ic_file
                                val iconTint =
                                    if (file.entryType == uniffi.connected_ffi.FfiFsEntryType.DIRECTORY) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurface
                                Icon(painterResource(iconRes), contentDescription = null, tint = iconTint)
                                Spacer(modifier = Modifier.width(8.dp))
                            }

                            Text(
                                text = file.name,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis
                            )
                        }
                        // Download button for both files and folders
                        Spacer(modifier = Modifier.width(8.dp))
                        if (!isDirectory) {
                            Text(
                                text = formatFileSize(file.size),
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant
                            )
                            Spacer(modifier = Modifier.width(8.dp))
                        }
                        IconButton(
                            onClick = {
                                app.getBrowsingDevice()?.let { device ->
                                    if (isDirectory) {
                                        app.downloadRemoteFolder(device, file.path)
                                    } else {
                                        app.downloadRemoteFile(device, file.path)
                                    }
                                }
                            },
                            enabled = downloadProgress == null
                        ) {
                            Icon(
                                painterResource(R.drawable.ic_download),
                                contentDescription = "Download",
                                tint = if (downloadProgress == null)
                                    MaterialTheme.colorScheme.primary
                                else
                                    MaterialTheme.colorScheme.onSurface.copy(alpha = 0.38f)
                            )
                        }
                    }
                }
            }
        }
    }
}

fun formatFileSize(bytes: ULong): String {
    if (bytes < 1024uL) return "$bytes B"
    val kb = bytes.toDouble() / 1024.0
    if (kb < 1024.0) return String.format(Locale.getDefault(), "%.1f KB", kb)
    val mb = kb / 1024.0
    if (mb < 1024.0) return String.format(Locale.getDefault(), "%.1f MB", mb)
    val gb = mb / 1024.0
    return String.format(Locale.getDefault(), "%.1f GB", gb)
}

fun getDeviceIcon(type: DeviceType, name: String? = null): Int {
    return when (type) {
        DeviceType.ANDROID -> R.drawable.ic_android
        DeviceType.IOS -> R.drawable.ic_ios
        DeviceType.LINUX -> R.drawable.ic_linux
        DeviceType.WINDOWS -> R.drawable.ic_windows
        DeviceType.MAC_OS -> R.drawable.ic_macos
        DeviceType.UNKNOWN -> {
            // Fall back to name-based detection
            val normalized = name?.trim()?.lowercase() ?: ""
            when {
                normalized.contains("phone") || normalized.contains("mobile") -> R.drawable.ic_android
                normalized.contains("tablet") -> R.drawable.ic_tablet
                normalized.contains("laptop") || normalized.contains("notebook") -> R.drawable.ic_laptop
                normalized.contains("desktop") || normalized.contains("computer") || normalized.contains("pc") -> R.drawable.ic_desktop
                normalized.contains("tv") || normalized.contains("television") -> R.drawable.ic_tv
                normalized.contains("watch") || normalized.contains("wearable") -> R.drawable.ic_watch
                else -> R.drawable.ic_device_unknown
            }
        }
    }
}

enum class Screen {
    Home,
    Settings
}

enum class PhoneDataKind(val title: String) {
    Contacts("Contacts"),
    Conversations("Conversations"),
    CallLog("Call Log")
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MainAppNavigation(
    connectedApp: ConnectedApp,
    filePickerLauncher: ActivityResultLauncher<String>? = null,
    folderPickerLauncher: ActivityResultLauncher<Uri?>? = null,
    sendFolderLauncher: ActivityResultLauncher<Uri?>? = null,
    downloadDirPickerLauncher: ActivityResultLauncher<Uri?>? = null
) {
    var currentScreen by remember { mutableStateOf(Screen.Home) }
    val snackbarHostState = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()

    LaunchedEffect(connectedApp.unpairNotification.value) {
        connectedApp.unpairNotification.value?.let { message ->
            scope.launch {
                snackbarHostState.showSnackbar(
                    message = message,
                    duration = SnackbarDuration.Short
                )
            }
            connectedApp.unpairNotification.value = null
        }
    }

    Scaffold(
        snackbarHost = { SnackbarHost(snackbarHostState) },
        bottomBar = {
            NavigationBar(
                containerColor = MaterialTheme.colorScheme.surface,
                contentColor = MaterialTheme.colorScheme.onSurface
            ) {
                NavigationBarItem(
                    icon = { Icon(painterResource(R.drawable.ic_nav_devices), contentDescription = "Devices") },
                    label = { Text("Devices") },
                    selected = currentScreen == Screen.Home,
                    onClick = { currentScreen = Screen.Home },
                    colors = NavigationBarItemDefaults.colors(
                        selectedIconColor = MaterialTheme.colorScheme.onSecondaryContainer,
                        selectedTextColor = MaterialTheme.colorScheme.onSecondaryContainer,
                        indicatorColor = MaterialTheme.colorScheme.secondaryContainer,
                        unselectedIconColor = MaterialTheme.colorScheme.onSurfaceVariant,
                        unselectedTextColor = MaterialTheme.colorScheme.onSurfaceVariant
                    )
                )
                NavigationBarItem(
                    icon = { Icon(painterResource(R.drawable.ic_nav_settings), contentDescription = "Settings") },
                    label = { Text("Settings") },
                    selected = currentScreen == Screen.Settings,
                    onClick = {
                        currentScreen = Screen.Settings
                    },
                    colors = NavigationBarItemDefaults.colors(
                        selectedIconColor = MaterialTheme.colorScheme.onSecondaryContainer,
                        selectedTextColor = MaterialTheme.colorScheme.onSecondaryContainer,
                        indicatorColor = MaterialTheme.colorScheme.secondaryContainer,
                        unselectedIconColor = MaterialTheme.colorScheme.onSurfaceVariant,
                        unselectedTextColor = MaterialTheme.colorScheme.onSurfaceVariant
                    )
                )
            }
        }
    ) { paddingValues ->
        Box(modifier = Modifier.padding(paddingValues)) {
            when (currentScreen) {
                Screen.Home -> HomeScreen(connectedApp, filePickerLauncher, sendFolderLauncher)
                Screen.Settings -> SettingsScreen(connectedApp, folderPickerLauncher, downloadDirPickerLauncher)
            }
        }

        if (connectedApp.pendingShareUris.isNotEmpty()) {
            val shareCount = connectedApp.pendingShareUris.size
            AlertDialog(
                onDismissRequest = { connectedApp.clearPendingShare() },
                title = { Text("Send to device") },
                text = {
                    Column {
                        Text(
                            if (shareCount == 1) "Choose a device for 1 item."
                            else "Choose a device for $shareCount items."
                        )
                        Spacer(modifier = Modifier.height(12.dp))
                        if (connectedApp.devices.isEmpty()) {
                            Text("No devices available.")
                        } else {
                            LazyColumn(modifier = Modifier.heightIn(max = 320.dp)) {
                                items(connectedApp.devices) { device ->
                                    Row(
                                        modifier = Modifier
                                            .fillMaxWidth()
                                            .clickable { connectedApp.sendPendingShareToDevice(device) }
                                            .padding(vertical = 8.dp),
                                        verticalAlignment = Alignment.CenterVertically
                                    ) {
                                        Icon(
                                            painterResource(getDeviceIcon(device.deviceType, device.name)),
                                            contentDescription = null,
                                            modifier = Modifier.size(20.dp),
                                            tint = MaterialTheme.colorScheme.primary
                                        )
                                        Spacer(modifier = Modifier.width(8.dp))
                                        Text(device.name)
                                    }
                                }
                            }
                        }
                    }
                },
                confirmButton = {
                    TextButton(onClick = { connectedApp.clearPendingShare() }) {
                        Text("Cancel")
                    }
                }
            )
        }

        if (connectedApp.pendingShareUrl.value != null) {
            val url = connectedApp.pendingShareUrl.value!!
            AlertDialog(
                onDismissRequest = { connectedApp.clearPendingShareUrl() },
                title = { Text("Open Link on Desktop") },
                text = {
                    Column {
                        Text(
                            url,
                            maxLines = 2,
                            overflow = TextOverflow.Ellipsis,
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant
                        )
                        Spacer(modifier = Modifier.height(12.dp))
                        Text("Select a connected desktop device:")
                        Spacer(modifier = Modifier.height(8.dp))
                        if (connectedApp.devices.isEmpty()) {
                            Text("No devices available.")
                        } else {
                            LazyColumn(modifier = Modifier.heightIn(max = 320.dp)) {
                                items(connectedApp.devices) { device ->
                                    Row(
                                        modifier = Modifier
                                            .fillMaxWidth()
                                            .clickable { connectedApp.sendPendingShareUrlToDevice(device) }
                                            .padding(vertical = 8.dp),
                                        verticalAlignment = Alignment.CenterVertically
                                    ) {
                                        Icon(
                                            painterResource(getDeviceIcon(device.deviceType, device.name)),
                                            contentDescription = null,
                                            modifier = Modifier.size(20.dp),
                                            tint = MaterialTheme.colorScheme.primary
                                        )
                                        Spacer(modifier = Modifier.width(8.dp))
                                        Text(device.name)
                                    }
                                }
                            }
                        }
                    }
                },
                confirmButton = {
                    TextButton(onClick = { connectedApp.clearPendingShareUrl() }) {
                        Text("Cancel")
                    }
                }
            )
        }

        // Pairing Request Dialog
        if (connectedApp.pairingRequest.value != null) {
            val request = connectedApp.pairingRequest.value!!
            AlertDialog(
                onDismissRequest = { connectedApp.rejectDevice(request) },
                title = { Text("Pairing Request") },
                text = { Text("${request.deviceName} wants to pair.\nFingerprint: ${request.fingerprint}") },
                confirmButton = {
                    Button(onClick = { connectedApp.trustDevice(request) }) {
                        Text("Trust")
                    }
                },
                dismissButton = {
                    Button(onClick = { connectedApp.rejectDevice(request) }) {
                        Text("Reject")
                    }
                }
            )
        }

        // Transfer Request Dialog
        if (connectedApp.transferRequest.value != null) {
            val request = connectedApp.transferRequest.value!!
            val pendingCount = connectedApp.pendingTransferRequests.size
            val pendingDevices = connectedApp.pendingTransferRequests
                .map { it.fromDevice }
                .distinct()
                .size
            AlertDialog(
                onDismissRequest = { connectedApp.rejectTransfer(request) },
                title = {
                    Text(if (pendingCount > 1) "Incoming Files" else "Incoming File")
                },
                text = {
                    Column {
                        if (pendingCount > 1) {
                            Text("All devices: $pendingDevices")
                            Text("Pending requests: $pendingCount")
                            Spacer(modifier = Modifier.height(8.dp))
                        }
                        Text("${request.fromDevice} wants to send:\n${request.filename}\nSize: ${request.fileSize} bytes")
                    }
                },
                confirmButton = {
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        if (pendingCount > 1) {
                            TextButton(onClick = { connectedApp.acceptAllTransfers() }) {
                                Text("Accept all")
                            }
                        }
                        Button(onClick = { connectedApp.acceptTransfer(request) }) {
                            Text("Accept")
                        }
                    }
                },
                dismissButton = {
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        if (pendingCount > 1) {
                            TextButton(onClick = { connectedApp.rejectAllTransfers() }) {
                                Text("Reject all")
                            }
                        }
                        Button(onClick = { connectedApp.rejectTransfer(request) }) {
                            Text("Reject")
                        }
                    }
                }
            )
        }
    }
}

@Composable
fun NotificationWarningCard(packageName: String) {
    val context = LocalContext.current
    Box(
        modifier = Modifier
            .fillMaxWidth()
            .padding(bottom = 16.dp)
            .background(MaterialTheme.colorScheme.errorContainer, MaterialTheme.shapes.medium)
            .clickable {
                val intent = Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).apply {
                    putExtra(Settings.EXTRA_APP_PACKAGE, packageName)
                }
                context.startActivity(intent)
            }
            .padding(16.dp)
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Icon(
                Icons.Filled.Warning,
                contentDescription = "Warning",
                tint = MaterialTheme.colorScheme.onErrorContainer,
                modifier = Modifier.size(24.dp)
            )
            Spacer(modifier = Modifier.width(16.dp))
            Column {
                Text(
                    "Notifications Disabled",
                    style = MaterialTheme.typography.titleSmall,
                    color = MaterialTheme.colorScheme.onErrorContainer
                )
                Text(
                    "Enable notifications to receive files.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onErrorContainer
                )
            }
        }
    }
}

@Composable
fun HomeScreen(
    connectedApp: ConnectedApp,
    filePickerLauncher: ActivityResultLauncher<String>? = null,
    sendFolderLauncher: ActivityResultLauncher<Uri?>? = null
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var areNotificationsEnabled by remember { mutableStateOf(true) }
    var isRefreshing by remember { mutableStateOf(false) }
    var pullOffset by remember { mutableStateOf(0f) }
    val refreshThreshold = 160f

    fun triggerRefresh() {
        if (isRefreshing) return
        isRefreshing = true
        pullOffset = 0f
        scope.launch {
            connectedApp.refreshDeviceDiscovery()
            delay(1200)
            isRefreshing = false
        }
    }

    val nestedScrollConnection = remember {
        object : NestedScrollConnection {
            override fun onPreScroll(available: Offset, source: NestedScrollSource): Offset {
                if (pullOffset > 0f && available.y < 0f) {
                    val consumed = available.y.coerceAtLeast(-pullOffset)
                    pullOffset += consumed
                    return Offset(0f, consumed)
                }
                return Offset.Zero
            }

            override fun onPostScroll(
                consumed: Offset,
                available: Offset,
                source: NestedScrollSource
            ): Offset {
                if (available.y > 0f && source == NestedScrollSource.UserInput) {
                    pullOffset = (pullOffset + available.y * 0.5f).coerceAtMost(refreshThreshold * 1.5f)
                    return Offset(0f, available.y)
                }
                return Offset.Zero
            }

            override suspend fun onPreFling(available: Velocity): Velocity {
                if (!isRefreshing && pullOffset > 0f) {
                    if (pullOffset >= refreshThreshold) {
                        triggerRefresh()
                    } else {
                        pullOffset = 0f
                    }
                    return available
                }
                return Velocity.Zero
            }

            override suspend fun onPostFling(consumed: Velocity, available: Velocity): Velocity {
                if (!isRefreshing && pullOffset > 0f) {
                    pullOffset = 0f
                }
                return Velocity.Zero
            }
        }
    }

    val lifecycleOwner = androidx.lifecycle.compose.LocalLifecycleOwner.current
    DisposableEffect(lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) {
                val notificationManager =
                    context.getSystemService(Context.NOTIFICATION_SERVICE) as android.app.NotificationManager
                areNotificationsEnabled = notificationManager.areNotificationsEnabled()
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose {
            lifecycleOwner.lifecycle.removeObserver(observer)
        }
    }

    Column(modifier = Modifier.fillMaxSize().padding(16.dp)) {
        Text(
            "Nearby Devices",
            style = MaterialTheme.typography.headlineMedium,
            modifier = Modifier.padding(bottom = 16.dp)
        )

        if (!areNotificationsEnabled) {
            NotificationWarningCard(context.packageName)
        }

        Box(
            modifier = Modifier
                .weight(1f)
                .nestedScroll(nestedScrollConnection)
        ) {
            Column(modifier = Modifier.fillMaxSize()) {
                if (pullOffset > 0f || isRefreshing) {
                    Box(
                        modifier = Modifier
                            .fillMaxWidth()
                            .height((pullOffset / refreshThreshold * 56f).coerceAtMost(56f).dp),
                        contentAlignment = Alignment.Center
                    ) {
                        if (isRefreshing) {
                            CircularProgressIndicator(modifier = Modifier.size(24.dp))
                        } else {
                            val progress = (pullOffset / refreshThreshold).coerceIn(0f, 1f)
                            CircularProgressIndicator(
                                progress = { progress },
                                modifier = Modifier.size(24.dp)
                            )
                        }
                    }
                }

                if (connectedApp.devices.isEmpty()) {
                    Box(
                        modifier = Modifier.fillMaxSize(),
                        contentAlignment = Alignment.Center
                    ) {
                        Column(horizontalAlignment = Alignment.CenterHorizontally) {
                            Icon(
                                painterResource(R.drawable.ic_nav_devices),
                                contentDescription = "Searching",
                                modifier = Modifier.size(64.dp),
                                tint = MaterialTheme.colorScheme.primary.copy(alpha = 0.6f)
                            )
                            Spacer(modifier = Modifier.height(16.dp))
                            Text(
                                "Searching for devices...",
                                style = MaterialTheme.typography.bodyLarge,
                                color = MaterialTheme.colorScheme.onSurfaceVariant
                            )
                            Spacer(modifier = Modifier.height(8.dp))
                            CircularProgressIndicator(modifier = Modifier.size(24.dp))
                        }
                    }
                } else {
                    LazyColumn(modifier = Modifier.fillMaxSize()) {
                        items(connectedApp.devices) { device ->
                            DeviceItem(device, connectedApp, filePickerLauncher, sendFolderLauncher)
                        }
                    }
                }
            }
        }

        // Compression progress at bottom
        val compressionProgress = connectedApp.compressionProgress.value
        if (compressionProgress != null) {
            Card(
                colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.secondaryContainer),
                modifier = Modifier.fillMaxWidth().padding(top = 8.dp)
            ) {
                Column(modifier = Modifier.padding(12.dp)) {
                    Text(
                        "Compressing folder...",
                        style = MaterialTheme.typography.titleSmall,
                        fontWeight = androidx.compose.ui.text.font.FontWeight.Bold
                    )
                    Spacer(modifier = Modifier.height(4.dp))
                    Text(
                        compressionProgress.currentFile,
                        style = MaterialTheme.typography.bodySmall,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis
                    )
                    Spacer(modifier = Modifier.height(8.dp))
                    LinearProgressIndicator(
                        progress = { compressionProgress.percentComplete / 100f },
                        modifier = Modifier.fillMaxWidth().height(6.dp),
                        color = MaterialTheme.colorScheme.primary,
                        trackColor = MaterialTheme.colorScheme.surfaceVariant
                    )
                    Spacer(modifier = Modifier.height(4.dp))
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.SpaceBetween
                    ) {
                        Text(
                            "${compressionProgress.filesProcessed}/${compressionProgress.totalFiles} files",
                            style = MaterialTheme.typography.bodySmall
                        )
                        Text(
                            "${formatBytes(compressionProgress.bytesProcessed)}/${formatBytes(compressionProgress.totalBytes)}",
                            style = MaterialTheme.typography.bodySmall
                        )
                    }
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.SpaceBetween
                    ) {
                        Text(
                            "${formatBytes(compressionProgress.speedBytesPerSec)}/s",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.primary
                        )
                        if (compressionProgress.estimatedSecondsRemaining > 0) {
                            Text(
                                "~${formatDuration(compressionProgress.estimatedSecondsRemaining)} remaining",
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant
                            )
                        }
                    }
                }
            }
        }

        // Transfer status at bottom
        if (compressionProgress == null && connectedApp.transferStatus.value.isNotEmpty() && connectedApp.transferStatus.value != "Idle") {
            Card(
                colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.primaryContainer),
                modifier = Modifier.fillMaxWidth().padding(top = 8.dp)
            ) {
                Row(
                    modifier = Modifier.padding(12.dp).fillMaxWidth(),
                    horizontalArrangement = Arrangement.SpaceBetween,
                    verticalAlignment = Alignment.CenterVertically
                ) {
                    Text(
                        connectedApp.transferStatus.value,
                        style = MaterialTheme.typography.bodySmall,
                        modifier = Modifier.weight(1f)
                    )
                    // Show cancel button for outgoing or incoming transfers
                    if (connectedApp.activeTransferId != null || connectedApp.activeIncomingTransferId != null) {
                        TextButton(
                            onClick = { connectedApp.cancelFileTransfer() }
                        ) {
                            Icon(
                                Icons.Default.Close,
                                contentDescription = "Cancel Transfer",
                                tint = MaterialTheme.colorScheme.error
                            )
                            Spacer(modifier = Modifier.width(4.dp))
                            Text("Cancel")
                        }
                    }
                }
            }
        }
    }
}

private fun formatBytes(bytes: Long): String {
    return when {
        bytes >= 1_073_741_824 -> String.format(Locale.getDefault(), "%.1f GB", bytes / 1_073_741_824.0)
        bytes >= 1_048_576 -> String.format(Locale.getDefault(), "%.1f MB", bytes / 1_048_576.0)
        bytes >= 1024 -> String.format(Locale.getDefault(), "%.1f KB", bytes / 1024.0)
        else -> "$bytes B"
    }
}

private fun formatDuration(seconds: Long): String {
    return when {
        seconds >= 3600 -> String.format(
            Locale.getDefault(),
            "%d:%02d:%02d",
            seconds / 3600,
            (seconds % 3600) / 60,
            seconds % 60
        )

        seconds >= 60 -> String.format(Locale.getDefault(), "%d:%02d", seconds / 60, seconds % 60)
        else -> "${seconds}s"
    }
}

@SuppressLint("BatteryLife")
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsScreen(
    connectedApp: ConnectedApp,
    folderPickerLauncher: ActivityResultLauncher<Uri?>? = null,
    downloadDirPickerLauncher: ActivityResultLauncher<Uri?>? = null
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val lifecycleOwner = androidx.lifecycle.compose.LocalLifecycleOwner.current
    var isNotificationAccessGranted by remember { mutableStateOf(false) }
    var isBackgroundServiceRunning by remember {
        mutableStateOf(ConnectedService.isRunning)
    }

    var updateInfo by remember { mutableStateOf<UpdateInfo?>(null) }
    var isCheckingUpdate by remember { mutableStateOf(false) }
    val currentVersion = remember {
        try {
            context.packageManager.getPackageInfo(context.packageName, 0).versionName ?: "Unknown"
        } catch (e: Exception) {
            "Unknown"
        }
    }

    LaunchedEffect(Unit) {
        // Self-update check is sideload-flavor only; playStore builds update via Google Play.
        if (AppUpdater.isSelfUpdateAllowed(context) && currentVersion != "Unknown") {
            isCheckingUpdate = true
            updateInfo = AppUpdater.checkForUpdate(currentVersion)
            isCheckingUpdate = false
        }
    }

    // Battery Optimization
    val powerManager = context.getSystemService(Context.POWER_SERVICE) as android.os.PowerManager
    var isIgnoringBatteryOptimizations by remember {
        mutableStateOf(
            powerManager.isIgnoringBatteryOptimizations(context.packageName)
        )
    }

    // Telephony permissions
    /* Phone Link / Telephony - Commented out for Play Store compliance (uncomment to re-enable)
    var hasTelephonyPermissions by remember { mutableStateOf(false) }
    var permissionsRequested by remember { mutableStateOf(false) }
    */
    var pendingFullAccess by remember { mutableStateOf(false) }

    DisposableEffect(lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) {
                // Check background service
                isBackgroundServiceRunning = ConnectedService.isRunning

                // Check battery optimizations
                isIgnoringBatteryOptimizations = powerManager.isIgnoringBatteryOptimizations(context.packageName)

                // Check notification access using robust method
                isNotificationAccessGranted = isNotificationListenerEnabled(context, MediaObserverService::class.java)

                /* Phone Link / Telephony permissions - Commented out for Play Store compliance (uncomment to re-enable)
                hasTelephonyPermissions = connectedApp.telephonyProvider.hasContactsPermission() &&
                        connectedApp.telephonyProvider.hasSmsPermission() &&
                        connectedApp.telephonyProvider.hasCallLogPermission()
                */

                // Check pending full access
                if (pendingFullAccess && connectedApp.isFullAccessGranted()) {
                    pendingFullAccess = false
                    connectedApp.setFullAccess()
                }
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose {
            lifecycleOwner.lifecycle.removeObserver(observer)
        }
    }
    /* Phone Link permission helpers - Commented out for Play Store compliance (uncomment to re-enable)
    val activity = context as? ComponentActivity

    fun getMissingPermissions(): Array<String> {
        return connectedApp.telephonyProvider.getRequiredPermissions().filter { permission ->
            ContextCompat.checkSelfPermission(context, permission) !=
                    PackageManager.PERMISSION_GRANTED
        }.toTypedArray()
    }

    fun shouldOpenSettings(): Boolean {
        if (activity == null) return false
        // Only open settings if we've already tried requesting and user denied with "Don't ask again"
        if (!permissionsRequested) return false
        val missingPermissions = getMissingPermissions()
        // If all missing permissions have rationale disabled, user selected "Don't ask again"
        return missingPermissions.isNotEmpty() && missingPermissions.all { permission ->
            !activity.shouldShowRequestPermissionRationale(permission)
        }
    }
    */

    fun openAppPermissionSettings() {
        val intent = Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS).apply {
            data = Uri.fromParts("package", context.packageName, null)
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        }
        context.startActivity(intent)
    }

    /* Phone Link permission launcher & disclosure - Commented out for Play Store compliance (uncomment to re-enable)
    val telephonyPermissionLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions()
    ) { permissions ->
        permissionsRequested = true
        hasTelephonyPermissions = permissions.values.all { it }
        if (hasTelephonyPermissions && !connectedApp.isTelephonyEnabled.value) {
            connectedApp.toggleTelephony()
        }
    }
    */

    val showRenameDialog = remember { mutableStateOf(false) }
    /* Phone Link disclosure dialog - Commented out for Play Store compliance (uncomment to re-enable)
    val showTelephonyDisclosureDialog = remember { mutableStateOf(false) }
    if (showTelephonyDisclosureDialog.value) {
        AlertDialog(
            onDismissRequest = {
                showTelephonyDisclosureDialog.value = false
            },
            title = { Text("Phone Link Permission") },
            text = {
                Text(
                    "Connected requires Contacts, SMS, and Call Log permissions to synchronize your messages and call history with your linked desktop device.\n\n" +
                    "This allows you to read, receive, and reply to SMS messages, as well as view your call history directly from your computer.\n\n" +
                    "Phone Link is optional and can be disabled at any time. Data is accessed only to provide the enabled synchronization feature.\n\n" +
                    "All data is transferred directly between your devices over your local network using end-to-end encryption. No data is ever sent to remote servers or third parties."
                )
            },
            confirmButton = {
                Button(onClick = {
                    showTelephonyDisclosureDialog.value = false
                    telephonyPermissionLauncher.launch(
                        connectedApp.telephonyProvider.getRequiredPermissions()
                    )
                }) {
                    Text("Grant")
                }
            },
            dismissButton = {
                Button(onClick = {
                    showTelephonyDisclosureDialog.value = false
                }) {
                    Text("Cancel")
                }
            }
        )
    }
    */
    val deviceName = remember { mutableStateOf(connectedApp.getDeviceName()) }

    if (showRenameDialog.value) {
        var newName by remember { mutableStateOf(deviceName.value) }
        AlertDialog(
            onDismissRequest = {
                showRenameDialog.value = false
            },
            title = { Text("Rename Device") },
            text = {
                OutlinedTextField(
                    value = newName,
                    onValueChange = {
                        newName = it
                    },
                    label = { Text("Device Name") },
                    singleLine = true
                )
            },
            confirmButton = {
                Button(onClick = {
                    val trimmedName = newName.trim()
                    if (trimmedName.isNotBlank()) {
                        connectedApp.renameDevice(trimmedName)
                        deviceName.value = trimmedName
                        showRenameDialog.value = false
                    }
                }) {
                    Text("Save")
                }
            },
            dismissButton = {
                Button(onClick = {
                    showRenameDialog.value = false
                }) {
                    Text("Cancel")
                }
            }
        )
    }

    LazyColumn(
        modifier = Modifier.fillMaxSize().padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp)
    ) {
        item {
            Text(
                "Settings",
                style = MaterialTheme.typography.headlineMedium,
                modifier = Modifier.padding(bottom = 8.dp)
            )
        }

        // Device Name Section
        item {
            Card(
                colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant),
                modifier = Modifier.fillMaxWidth()
            ) {
                Column(modifier = Modifier.padding(16.dp)) {
                    Text("Device Name", style = MaterialTheme.typography.titleMedium)
                    Text(
                        "This name will be visible to other devices",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant
                    )
                    Spacer(modifier = Modifier.height(12.dp))
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.SpaceBetween
                    ) {
                        Text(
                            deviceName.value,
                            style = MaterialTheme.typography.bodyLarge,
                            modifier = Modifier.padding(start = 4.dp)
                        )
                        Button(onClick = {
                            showRenameDialog.value = true
                        }) {
                            Text("Rename")
                        }
                    }
                }
            }
        }

        // Paired Devices Section
        item {
            val trustedIds = connectedApp.trustedDevices.toList()
            val trustedPeers = try {
                getTrustedPeers()
            } catch (_: Exception) {
                emptyList()
            }
            val peerNames = trustedPeers.associate { it.deviceId to it.name }
            Card(
                colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant),
                modifier = Modifier.fillMaxWidth()
            ) {
                Column(modifier = Modifier.padding(16.dp)) {
                    Text("Paired Devices", style = MaterialTheme.typography.titleMedium)
                    Text(
                        "Manage devices that have been paired with this device",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant
                    )
                    Spacer(modifier = Modifier.height(12.dp))
                    if (trustedIds.isEmpty()) {
                        Text(
                            "No devices paired yet",
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.padding(start = 4.dp)
                        )
                    } else {
                        trustedIds.forEach { deviceId ->
                            val onlineDevice = connectedApp.devices.firstOrNull { it.id == deviceId }
                            val deviceName =
                                onlineDevice?.name ?: peerNames[deviceId]?.takeIf { it.isNotEmpty() } ?: deviceId
                            val deviceType = onlineDevice?.deviceType?.displayName() ?: "Offline"
                            val isOnline = onlineDevice != null
                            Row(
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .padding(vertical = 6.dp),
                                verticalAlignment = Alignment.CenterVertically,
                                horizontalArrangement = Arrangement.SpaceBetween
                            ) {
                                Row(
                                    verticalAlignment = Alignment.CenterVertically,
                                    modifier = Modifier.weight(1f)
                                ) {
                                    Box(
                                        modifier = Modifier
                                            .size(8.dp)
                                            .background(
                                                color = if (isOnline) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant,
                                                shape = CircleShape
                                            )
                                    )
                                    Spacer(modifier = Modifier.width(12.dp))
                                    Column {
                                        Text(
                                            deviceName,
                                            style = MaterialTheme.typography.bodyLarge
                                        )
                                        Text(
                                            if (isOnline) deviceType else "Offline",
                                            style = MaterialTheme.typography.bodySmall,
                                            color = MaterialTheme.colorScheme.onSurfaceVariant
                                        )
                                    }
                                }
                                Row {
                                    TextButton(
                                        onClick = { connectedApp.unpairDeviceById(deviceId) },
                                        colors = ButtonDefaults.textButtonColors(
                                            contentColor = MaterialTheme.colorScheme.error
                                        )
                                    ) {
                                        Text("Unpair")
                                    }
                                }
                            }
                            if (deviceId != trustedIds.last()) {
                                HorizontalDivider(modifier = Modifier.padding(vertical = 4.dp))
                            }
                        }
                    }
                }
            }
        }

        // Download Directory Section
        item {
            Card(
                colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant),
                modifier = Modifier.fillMaxWidth()
            ) {
                Column(modifier = Modifier.padding(16.dp)) {
                    Text("Download Location", style = MaterialTheme.typography.titleMedium)
                    Text(
                        "Choose where received files are saved",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant
                    )
                    Spacer(modifier = Modifier.height(12.dp))
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.SpaceBetween
                    ) {
                        Text(
                            connectedApp.getDownloadDirDisplayName(),
                            style = MaterialTheme.typography.bodyLarge,
                            modifier = Modifier.weight(1f).padding(start = 4.dp),
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis
                        )
                        Spacer(modifier = Modifier.width(8.dp))
                        Button(onClick = {
                            downloadDirPickerLauncher?.launch(null)
                        }) {
                            Text("Browse")
                        }
                    }
                    if (connectedApp.customDownloadUri.value != null) {
                        Spacer(modifier = Modifier.height(8.dp))
                        OutlinedButton(
                            onClick = { connectedApp.setCustomDownloadDir(null) },
                            modifier = Modifier.fillMaxWidth()
                        ) {
                            Text("Reset to Default (Downloads)")
                        }
                    }
                }
            }
        }

        // Run in Background Section
        item {
            var isStoppingService by remember { mutableStateOf(false) }

            Card(
                colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant),
                modifier = Modifier.fillMaxWidth()
            ) {
                Column(modifier = Modifier.padding(16.dp)) {
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        verticalAlignment = Alignment.CenterVertically
                    ) {
                        Icon(
                            painterResource(R.drawable.ic_refresh), // Using sync/refresh icon as placeholder
                            contentDescription = null,
                            tint = MaterialTheme.colorScheme.primary,
                            modifier = Modifier.size(24.dp)
                        )
                        Spacer(modifier = Modifier.width(16.dp))
                        Column(modifier = Modifier.weight(1f)) {
                            Text("Run in Background", style = MaterialTheme.typography.titleMedium)
                            Text(
                                "Keep app running to receive files and share clipboard anytime",
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant
                            )
                        }
                        Switch(
                            checked = isBackgroundServiceRunning && !isStoppingService,
                            enabled = !isStoppingService,
                            onCheckedChange = { enabled ->
                                try {
                                    if (enabled) {
                                        val intent = Intent(context, ConnectedService::class.java)
                                        context.startForegroundService(intent)
                                        isBackgroundServiceRunning = true
                                    } else {
                                        // Set stopping state to prevent UI crashes
                                        isStoppingService = true
                                        val intent = Intent(context, ConnectedService::class.java)
                                        context.stopService(intent)
                                        // Update state after a short delay to allow service to stop
                                        scope.launch {
                                            delay(500)
                                            isBackgroundServiceRunning = false
                                            isStoppingService = false
                                        }
                                    }
                                } catch (e: Exception) {
                                    android.util.Log.e("MainActivity", "Error toggling service: ${e.message}", e)
                                    isStoppingService = false
                                    // Revert state on error
                                    isBackgroundServiceRunning = ConnectedService.isRunning
                                }
                            }
                        )
                    }

                    if (!isIgnoringBatteryOptimizations) {
                        Spacer(modifier = Modifier.height(12.dp))
                        Button(
                            onClick = {
                                val intent = Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS).apply {
                                    data = "package:${context.packageName}".toUri()
                                    addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                                }
                                context.startActivity(intent)
                            },
                            colors = ButtonDefaults.buttonColors(
                                containerColor = MaterialTheme.colorScheme.secondaryContainer,
                                contentColor = MaterialTheme.colorScheme.onSecondaryContainer
                            ),
                            modifier = Modifier.fillMaxWidth()
                        ) {
                            Text("Disable Battery Optimizations")
                        }
                    }
                }
            }
        }

        // Media Control Section
        item {
            Card(
                colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant),
                modifier = Modifier.fillMaxWidth()
            ) {
                Column(modifier = Modifier.padding(16.dp)) {
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        verticalAlignment = Alignment.CenterVertically
                    ) {
                        Icon(
                            painterResource(R.drawable.ic_nav_media),
                            contentDescription = null,
                            tint = MaterialTheme.colorScheme.primary,
                            modifier = Modifier.size(24.dp)
                        )
                        Spacer(modifier = Modifier.width(16.dp))
                        Column(modifier = Modifier.weight(1f)) {
                            Text("Media Control", style = MaterialTheme.typography.titleMedium)
                            Text(
                                "Allow other devices to control media playback",
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant
                            )
                        }
                        Switch(
                            checked = connectedApp.isMediaControlEnabled.value,
                            onCheckedChange = { connectedApp.toggleMediaControl() }
                        )
                    }

                    // Notification Access Warning
                    if (!isNotificationAccessGranted && connectedApp.isMediaControlEnabled.value) {
                        Spacer(modifier = Modifier.height(12.dp))
                        Button(
                            onClick = {
                                openNotificationListenerSettings(context)
                            },
                            colors = ButtonDefaults.buttonColors(
                                containerColor = MaterialTheme.colorScheme.errorContainer,
                                contentColor = MaterialTheme.colorScheme.onErrorContainer
                            ),
                            modifier = Modifier.fillMaxWidth()
                        ) {
                            Text("⚠️ Grant Notification Access")
                        }

                        // Helper for Android 13+ Restricted Settings
                        Spacer(modifier = Modifier.height(4.dp))
                        TextButton(
                            onClick = { openAppPermissionSettings() },
                            modifier = Modifier.fillMaxWidth()
                        ) {
                            Text(
                                "Can't enable? Open App Settings and find Allow Restricted Access (usually in the 3 dots menu)",
                                style = MaterialTheme.typography.bodySmall
                            )
                        }
                    }
                }
            }
        }

        /* Phone Link / Telephony Section - Commented out for Play Store compliance (uncomment to re-enable)
        item {
            Card(
                colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant),
                modifier = Modifier.fillMaxWidth()
            ) {
                Column(modifier = Modifier.padding(16.dp)) {
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        verticalAlignment = Alignment.CenterVertically
                    ) {
                        Icon(
                            painterResource(R.drawable.ic_nav_phone),
                            contentDescription = null,
                            tint = MaterialTheme.colorScheme.primary,
                            modifier = Modifier.size(24.dp)
                        )
                        Spacer(modifier = Modifier.width(16.dp))
                        Column(modifier = Modifier.weight(1f)) {
                            Text("Phone Link", style = MaterialTheme.typography.titleMedium)
                            Text(
                                "SMS, call history, and contacts sync",
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant
                            )
                        }
                        Switch(
                            checked = connectedApp.isTelephonyEnabled.value,
                            onCheckedChange = { enabled ->
                                if (enabled && !hasTelephonyPermissions) {
                                    showTelephonyDisclosureDialog.value = true
                                } else {
                                    connectedApp.toggleTelephony()
                                }
                            }
                        )
                    }

                    Spacer(modifier = Modifier.height(12.dp))

                    // Permission status indicators
                    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        PermissionStatusRow(
                            label = "Contacts",
                            granted = connectedApp.telephonyProvider.hasContactsPermission()
                        )
                        PermissionStatusRow(
                            label = "SMS",
                            granted = connectedApp.telephonyProvider.hasSmsPermission()
                        )
                        PermissionStatusRow(
                            label = "Call Log",
                            granted = connectedApp.telephonyProvider.hasCallLogPermission()
                        )
                    }

                    if (!hasTelephonyPermissions) {
                        Spacer(modifier = Modifier.height(12.dp))
                        Button(
                            onClick = {
                                if (shouldOpenSettings()) {
                                    openAppPermissionSettings()
                                } else {
                                    showTelephonyDisclosureDialog.value = true
                                }
                            },
                            colors = ButtonDefaults.buttonColors(
                                containerColor = MaterialTheme.colorScheme.primaryContainer,
                                contentColor = MaterialTheme.colorScheme.onPrimaryContainer
                            ),
                            modifier = Modifier.fillMaxWidth()
                        ) {
                            Text(if (shouldOpenSettings()) "Open App Settings" else "Grant Permissions")
                        }
                    }

                }
            }
        }
        */

        // Shared Folder Section
        item {
            Card(
                colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant),
                modifier = Modifier.fillMaxWidth()
            ) {
                Column(modifier = Modifier.padding(16.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Icon(
                            painterResource(R.drawable.ic_folder),
                            contentDescription = null,
                            tint = MaterialTheme.colorScheme.primary,
                            modifier = Modifier.size(24.dp)
                        )
                        Spacer(modifier = Modifier.width(16.dp))
                        Text("Shared Folder", style = MaterialTheme.typography.titleMedium)
                    }
                    Spacer(modifier = Modifier.height(8.dp))

                    if (connectedApp.isFsProviderRegistered.value) {
                        Text(
                            "Currently sharing: ${connectedApp.sharedFolderName.value ?: "Unknown"}",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.primary
                        )
                    } else {
                        Text(
                            "No folder shared. Select a sharing mode below.",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant
                        )
                    }

                    Spacer(modifier = Modifier.height(16.dp))

                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                        Button(
                            onClick = {
                                if (connectedApp.isFullAccessGranted()) {
                                    connectedApp.setFullAccess()
                                } else {
                                    pendingFullAccess = true
                                    connectedApp.requestFullAccessPermission()
                                }
                            },
                            colors = ButtonDefaults.buttonColors(containerColor = MaterialTheme.colorScheme.tertiary),
                            modifier = Modifier.fillMaxWidth()
                        ) {
                            Text(if (connectedApp.isFullAccessGranted()) "Use Full Access" else "🔓 Grant Full Access")
                        }
                        Spacer(modifier = Modifier.height(8.dp))
                    }

                    OutlinedButton(
                        onClick = { folderPickerLauncher?.launch(null) },
                        modifier = Modifier.fillMaxWidth()
                    ) {
                        Text(if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) "Select Specific Folder" else "Select Folder")
                    }
                }
            }
        }

        // Troubleshooting Section
        if (connectedApp.isMediaControlEnabled.value && !isNotificationAccessGranted) {
            item {
                Card(
                    colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant),
                    modifier = Modifier.fillMaxWidth()
                ) {
                    Column(modifier = Modifier.padding(16.dp)) {
                        Text("Troubleshooting", style = MaterialTheme.typography.titleMedium)
                        Spacer(modifier = Modifier.height(8.dp))
                        Text(
                            "If you are unable to enable certain features or permissions (like 'Restricted Settings'), you may need to manually allow them in system settings.",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant
                        )
                        Spacer(modifier = Modifier.height(12.dp))
                        Button(
                            onClick = { openAppPermissionSettings() },
                            modifier = Modifier.fillMaxWidth(),
                            colors = ButtonDefaults.buttonColors(
                                containerColor = MaterialTheme.colorScheme.secondary,
                                contentColor = MaterialTheme.colorScheme.onSecondary
                            )
                        ) {
                            Text("Open App System Settings")
                        }
                    }
                }
            }
        }

        item {
            Card(
                colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant),
                modifier = Modifier.fillMaxWidth()
            ) {
                Column(modifier = Modifier.padding(16.dp)) {
                    Text("Appearance", style = MaterialTheme.typography.titleMedium)
                    Text(
                        "Choose how Connected looks on this device",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant
                    )
                    Spacer(modifier = Modifier.height(12.dp))

                    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        ThemeMode.entries.forEach { mode ->
                            val label = when (mode) {
                                ThemeMode.SYSTEM -> "System default"
                                ThemeMode.LIGHT -> "Light"
                                ThemeMode.DARK -> "Dark"
                            }

                            FilterChip(
                                selected = connectedApp.themeMode.value == mode,
                                onClick = { connectedApp.setThemeMode(mode) },
                                label = { Text(label) },
                                modifier = Modifier.fillMaxWidth()
                            )
                        }
                    }
                }
            }
        }

        item {
            Card(
                colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant),
                modifier = Modifier.fillMaxWidth()
            ) {
                Column(modifier = Modifier.padding(16.dp)) {
                    Text("About", style = MaterialTheme.typography.titleMedium)
                    Text(
                        "App Version: $currentVersion",
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant
                    )
                    Spacer(modifier = Modifier.height(12.dp))

                    if (BuildConfig.FLAVOR == "sideload") {
                        if (updateInfo != null) {
                            Button(
                                onClick = {
                                    updateInfo?.let {
                                        AppUpdater.downloadUpdate(context, it.downloadUrl, it.versionName)
                                    }
                                },
                                colors = ButtonDefaults.buttonColors(
                                    containerColor = MaterialTheme.colorScheme.primary,
                                    contentColor = MaterialTheme.colorScheme.onPrimary
                                ),
                                modifier = Modifier.fillMaxWidth()
                            ) {
                                Text("Update to ${updateInfo!!.versionName}")
                            }
                            Spacer(modifier = Modifier.height(8.dp))
                        } else if (isCheckingUpdate) {
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                CircularProgressIndicator(modifier = Modifier.size(16.dp), strokeWidth = 2.dp)
                                Spacer(modifier = Modifier.width(8.dp))
                                Text("Checking for updates...", style = MaterialTheme.typography.bodySmall)
                            }
                            Spacer(modifier = Modifier.height(8.dp))
                        }
                    } else {
                        // playStore flavor: no direct APK installs; send users to Play.
                        Button(
                            onClick = { AppUpdater.openPlayStoreListing(context) },
                            colors = ButtonDefaults.buttonColors(
                                containerColor = MaterialTheme.colorScheme.primary,
                                contentColor = MaterialTheme.colorScheme.onPrimary
                            ),
                            modifier = Modifier.fillMaxWidth()
                        ) {
                            Text("View in Play Store")
                        }
                        Spacer(modifier = Modifier.height(8.dp))
                    }

                    Button(
                        onClick = {
                            try {
                                val intent = Intent(
                                    Intent.ACTION_VIEW,
                                    "https://github.com/paterkleomenis/connected/issues/new".toUri()
                                )
                                intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                                context.startActivity(intent)
                            } catch (e: Exception) {
                                android.util.Log.e("MainActivity", "Failed to open feedback url", e)
                            }
                        },
                        colors = ButtonDefaults.buttonColors(
                            containerColor = MaterialTheme.colorScheme.primaryContainer,
                            contentColor = MaterialTheme.colorScheme.onPrimaryContainer
                        ),
                        modifier = Modifier.fillMaxWidth()
                    ) {
                        Text("Send Feedback")
                    }

                    Spacer(modifier = Modifier.height(8.dp))

                    OutlinedButton(
                        onClick = {
                            try {
                                val intent = Intent(
                                    Intent.ACTION_VIEW,
                                    "https://github.com/paterkleomenis/connected/blob/main/PRIVACY_POLICY.md".toUri()
                                )
                                intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                                context.startActivity(intent)
                            } catch (e: Exception) {
                                android.util.Log.e("MainActivity", "Failed to open privacy policy url", e)
                            }
                        },
                        modifier = Modifier.fillMaxWidth()
                    ) {
                        Text("Privacy Policy")
                    }
                }
            }
        }

    }
}

@Composable
fun PermissionStatusRow(label: String, granted: Boolean) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.SpaceBetween,
        verticalAlignment = Alignment.CenterVertically
    ) {
        Text(
            label,
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant
        )
        Text(
            if (granted) "✓ Granted" else "✗ Not granted",
            style = MaterialTheme.typography.bodySmall,
            color = if (granted) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.error
        )
    }
}

@Composable
private fun PhoneDataDialog(
    kind: PhoneDataKind,
    device: DiscoveredDevice,
    app: ConnectedApp,
    onDismiss: () -> Unit
) {
    val context = LocalContext.current
    var selectedConversation by remember(kind, device.id) { mutableStateOf<FfiConversation?>(null) }
    val title = selectedConversation?.let { conversationTitle(it) } ?: kind.title

    AlertDialog(
        onDismissRequest = onDismiss,
        title = {
            Text(
                title,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis
            )
        },
        text = {
            Column {
                Text(
                    device.name,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis
                )
                Spacer(modifier = Modifier.height(12.dp))
                if (kind == PhoneDataKind.Conversations && selectedConversation != null) {
                    ConversationMessagesList(device, selectedConversation!!, app)
                } else {
                    when (kind) {
                        PhoneDataKind.Contacts -> ContactsDataList(app.contacts, device.name, context)
                        PhoneDataKind.Conversations -> ConversationsDataList(
                            conversations = app.conversations,
                            deviceName = device.name,
                            onConversationClick = { conversation ->
                                if (app.requestMessagesFromDevice(device, conversation.id)) {
                                    selectedConversation = conversation
                                }
                            }
                        )

                        PhoneDataKind.CallLog -> CallLogDataList(app.callLog, device.name, context)
                    }
                }
            }
        },
        confirmButton = {
            TextButton(onClick = onDismiss) {
                Text("Done")
            }
        },
        dismissButton = {
            if (selectedConversation != null) {
                TextButton(onClick = { selectedConversation = null }) {
                    Text("Back")
                }
            }
        }
    )
}

@Composable
private fun ContactsDataList(contacts: List<FfiContact>, deviceName: String, context: Context) {
    if (contacts.isEmpty()) {
        PhoneDataWaiting("Waiting for contacts from $deviceName.")
        return
    }

    LazyColumn(modifier = Modifier.heightIn(max = 420.dp)) {
        items(contacts, key = { it.id }) { contact ->
            val phoneNumbers = contact.phoneNumbers.joinToString(", ") { phone ->
                "${formatEnumName(phone.label.name)}: ${phone.number}"
            }
            PhoneDataRow(
                title = contact.name,
                details = phoneDataDetails(
                    phoneNumbers,
                    contact.emails.joinToString(", ")
                ),
                onClick = {
                    copyTextToClipboard(context, "Contact", contactCopyValue(contact), "Copied contact")
                }
            )
        }
    }
}

@Composable
private fun ConversationsDataList(
    conversations: List<FfiConversation>,
    deviceName: String,
    onConversationClick: (FfiConversation) -> Unit
) {
    if (conversations.isEmpty()) {
        PhoneDataWaiting("Waiting for conversations from $deviceName.")
        return
    }

    LazyColumn(modifier = Modifier.heightIn(max = 420.dp)) {
        items(conversations, key = { it.id }) { conversation ->
            val unread = if (conversation.unreadCount > 0u) {
                "${conversation.unreadCount} unread"
            } else {
                null
            }
            PhoneDataRow(
                title = conversationTitle(conversation),
                details = phoneDataDetails(
                    conversation.lastMessage,
                    unread,
                    formatPhoneTimestamp(conversation.lastTimestamp)
                ),
                onClick = { onConversationClick(conversation) }
            )
        }
    }
}

@Composable
private fun CallLogDataList(callLog: List<FfiCallLogEntry>, deviceName: String, context: Context) {
    if (callLog.isEmpty()) {
        PhoneDataWaiting("Waiting for call log from $deviceName.")
        return
    }

    LazyColumn(modifier = Modifier.heightIn(max = 420.dp)) {
        items(callLog, key = { it.id }) { entry ->
            val displayName = entry.contactName?.takeIf { it.isNotBlank() }
                ?: entry.number.takeIf { it.isNotBlank() }
                ?: "Unknown Number"
            PhoneDataRow(
                title = displayName,
                details = phoneDataDetails(
                    "${formatEnumName(entry.callType.name)} - ${formatDuration(entry.duration.toLong())}",
                    formatPhoneTimestamp(entry.timestamp)
                ),
                onClick = { copyPhoneNumberToClipboard(context, entry.number) }
            )
        }
    }
}

@Composable
private fun ConversationMessagesList(
    device: DiscoveredDevice,
    conversation: FfiConversation,
    app: ConnectedApp
) {
    val listState = rememberLazyListState()
    val messages = app.currentMessages.filter { message -> message.threadId == conversation.id }
    val isCurrentThread = app.selectedConversationThreadId.value == conversation.id

    LaunchedEffect(conversation.id, messages.size, isCurrentThread) {
        if (isCurrentThread && messages.isNotEmpty()) {
            listState.scrollToItem(messages.lastIndex)
        }
    }

    if (!isCurrentThread || messages.isEmpty()) {
        PhoneDataWaiting("Loading conversation history from ${device.name}.")
        return
    }

    LazyColumn(
        modifier = Modifier.heightIn(max = 420.dp),
        state = listState
    ) {
        items(messages, key = { it.id }) { message ->
            MessageDataRow(message)
        }
    }
}

@Composable
private fun MessageDataRow(message: FfiSmsMessage) {
    Column(modifier = Modifier.fillMaxWidth().padding(vertical = 8.dp)) {
        Text(
            if (message.isOutgoing) "Me" else message.contactName?.takeIf { it.isNotBlank() } ?: message.address,
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis
        )
        Spacer(modifier = Modifier.height(2.dp))
        Text(
            message.body,
            style = MaterialTheme.typography.bodyMedium
        )
        formatPhoneTimestamp(message.timestamp)?.let { timestamp ->
            Spacer(modifier = Modifier.height(2.dp))
            Text(
                timestamp,
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant
            )
        }
    }
}

@Composable
private fun PhoneDataWaiting(message: String) {
    Row(
        modifier = Modifier.fillMaxWidth().padding(vertical = 16.dp),
        verticalAlignment = Alignment.CenterVertically
    ) {
        CircularProgressIndicator(modifier = Modifier.size(20.dp))
        Spacer(modifier = Modifier.width(12.dp))
        Text(
            message,
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant
        )
    }
}

@Composable
private fun PhoneDataRow(title: String, details: String?, onClick: (() -> Unit)? = null) {
    val modifier = if (onClick != null) {
        Modifier.fillMaxWidth().clickable(onClick = onClick).padding(vertical = 8.dp)
    } else {
        Modifier.fillMaxWidth().padding(vertical = 8.dp)
    }

    Column(modifier = modifier) {
        Text(
            title,
            style = MaterialTheme.typography.bodyMedium,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis
        )
        if (!details.isNullOrBlank()) {
            Spacer(modifier = Modifier.height(2.dp))
            Text(
                details,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 4,
                overflow = TextOverflow.Ellipsis
            )
        }
    }
}

private fun contactCopyValue(contact: FfiContact): String {
    val values = mutableListOf(contact.name)
    values.addAll(contact.phoneNumbers.map { it.number })
    values.addAll(contact.emails)
    return values
        .map { it.trim() }
        .filter { it.isNotEmpty() }
        .joinToString(", ")
}

private fun copyPhoneNumberToClipboard(context: Context, number: String) {
    val trimmed = number.trim()
    if (trimmed.isEmpty()) {
        showToast(context, "No phone number")
        return
    }

    copyTextToClipboard(context, "Phone Number", trimmed, "Copied $trimmed")
}

private fun copyTextToClipboard(context: Context, label: String, value: String, successMessage: String) {
    if (value.isBlank()) {
        showToast(context, "Nothing to copy")
        return
    }

    val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as android.content.ClipboardManager
    clipboard.setPrimaryClip(android.content.ClipData.newPlainText(label, value))
    showToast(context, successMessage)
}

private fun showToast(context: Context, message: String) {
    android.widget.Toast.makeText(context, message, android.widget.Toast.LENGTH_SHORT).show()
}

private fun conversationTitle(conversation: FfiConversation): String {
    if (conversation.contactNames.isNotEmpty()) return conversation.contactNames.joinToString(", ")
    if (conversation.addresses.isNotEmpty()) return conversation.addresses.joinToString(", ")
    return conversation.id
}

private fun phoneDataDetails(vararg values: String?): String? {
    val details = values
        .mapNotNull { it?.takeIf { value -> value.isNotBlank() } }
        .joinToString("\n")
    return details.takeIf { it.isNotBlank() }
}

private fun formatPhoneTimestamp(timestamp: ULong): String? {
    val raw = timestamp.toLong()
    if (raw <= 0L) return null

    val millis = if (raw > 10_000_000_000L) raw else raw * 1000L
    return android.text.format.DateFormat.format("MMM d, yyyy h:mm a", millis).toString()
}

private fun formatEnumName(raw: String): String {
    return raw.split("_").joinToString(" ") { part ->
        val lower = part.lowercase()
        if (lower.isEmpty()) lower else lower.substring(0, 1).uppercase() + lower.substring(1)
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DeviceItem(
    device: DiscoveredDevice,
    app: ConnectedApp,
    filePickerLauncher: ActivityResultLauncher<String>? = null,
    sendFolderLauncher: ActivityResultLauncher<Uri?>? = null
) {
    val isTrusted = app.trustedDevices.contains(device.id)
    val isPending = app.pendingPairing.contains(device.id)
    var showMenu by remember { mutableStateOf(false) }
    var showDetails by remember { mutableStateOf(false) }
    var showRemoteInput by remember { mutableStateOf(false) }
    val phoneDataKind = remember { mutableStateOf<PhoneDataKind?>(null) }

    Card(modifier = Modifier.padding(vertical = 4.dp).fillMaxWidth()) {
        Column(modifier = Modifier.padding(12.dp).fillMaxWidth()) {
            // Top row: Device info and action buttons
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically
            ) {
                Row(modifier = Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically) {
                    Icon(
                        painter = painterResource(getDeviceIcon(device.deviceType, device.name)),
                        contentDescription = null,
                        modifier = Modifier.size(20.dp),
                        tint = MaterialTheme.colorScheme.primary
                    )
                    Spacer(modifier = Modifier.width(10.dp))

                    Column(modifier = Modifier.clickable { showDetails = !showDetails }) {
                        Text(
                            text = device.name,
                            style = MaterialTheme.typography.bodyLarge,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis
                        )
                        if (showDetails) {
                            Text(text = "${device.ip}:${device.port}", style = MaterialTheme.typography.bodySmall)
                        }
                        if (isTrusted) {
                            Text(
                                text = "✓ Trusted",
                                style = MaterialTheme.typography.labelSmall,
                                color = MaterialTheme.colorScheme.primary
                            )
                        }
                    }
                }

                // Action buttons (right side)
                if (isTrusted) {
                    Row {
                        // Send file button
                        IconButton(onClick = {
                            app.setSelectedDeviceForFileTransfer(device)
                            filePickerLauncher?.launch("*/*")
                        }) {
                            Icon(painterResource(R.drawable.ic_send), contentDescription = "Send File")
                        }

                        // More options dropdown
                        Box {
                            IconButton(onClick = { showMenu = true }) {
                                Icon(painterResource(R.drawable.ic_settings), contentDescription = "Options")
                            }
                            DropdownMenu(
                                expanded = showMenu,
                                onDismissRequest = { showMenu = false }
                            ) {
                                DropdownMenuItem(
                                    text = { Text("Send Folder") },
                                    leadingIcon = {
                                        Icon(
                                            painterResource(R.drawable.ic_folder),
                                            contentDescription = null
                                        )
                                    },
                                    onClick = {
                                        showMenu = false
                                        app.setSelectedDeviceForFileTransfer(device)
                                        sendFolderLauncher?.launch(null)
                                    }
                                )
                                DropdownMenuItem(
                                    text = { Text("Share Clipboard") },
                                    leadingIcon = {
                                        Icon(
                                            painterResource(R.drawable.ic_nav_clipboard),
                                            contentDescription = null
                                        )
                                    },
                                    onClick = {
                                        showMenu = false
                                        app.sendClipboard(device)
                                    }
                                )
                                DropdownMenuItem(
                                    text = { Text("Browse Files") },
                                    leadingIcon = {
                                        Icon(
                                            painterResource(R.drawable.ic_folder),
                                            contentDescription = null
                                        )
                                    },
                                    onClick = {
                                        showMenu = false
                                        app.browseRemoteFiles(device)
                                    }
                                )
                                /* Phone Link actions - Commented out for Play Store compliance (uncomment to re-enable)
                                DropdownMenuItem(
                                    text = { Text("Request Contacts") },
                                    leadingIcon = {
                                        Icon(
                                            painterResource(R.drawable.ic_contact),
                                            contentDescription = null
                                        )
                                    },
                                    onClick = {
                                        showMenu = false
                                        if (app.requestContactsFromDevice(device)) {
                                            phoneDataKind.value = PhoneDataKind.Contacts
                                        }
                                    }
                                )
                                if (device.deviceType != DeviceType.IOS) {
                                    DropdownMenuItem(
                                        text = { Text("Request Conversations") },
                                        leadingIcon = {
                                            Icon(
                                                painterResource(R.drawable.ic_message),
                                                contentDescription = null
                                            )
                                        },
                                        onClick = {
                                            showMenu = false
                                            if (app.requestConversationsFromDevice(device)) {
                                                phoneDataKind.value = PhoneDataKind.Conversations
                                            }
                                        }
                                    )
                                    DropdownMenuItem(
                                        text = { Text("Request Call Log") },
                                        leadingIcon = {
                                            Icon(
                                                painterResource(R.drawable.ic_call),
                                                contentDescription = null
                                            )
                                        },
                                        onClick = {
                                            showMenu = false
                                            if (app.requestCallLogFromDevice(device)) {
                                                phoneDataKind.value = PhoneDataKind.CallLog
                                            }
                                        }
                                    )
                                }
                                */
                                // Remote input and power/session commands — only
                                // meaningful for desktop peers.
                                if (device.deviceType == DeviceType.LINUX
                                    || device.deviceType == DeviceType.WINDOWS
                                    || device.deviceType == DeviceType.MAC_OS
                                ) {
                                    HorizontalDivider()
                                    DropdownMenuItem(
                                        text = { Text("Remote Input") },
                                        leadingIcon = {
                                            Icon(
                                                painterResource(R.drawable.ic_desktop),
                                                contentDescription = null
                                            )
                                        },
                                        onClick = {
                                            showMenu = false
                                            showRemoteInput = true
                                        }
                                    )
                                    var showRemoteMenu by remember { mutableStateOf(false) }
                                    Box {
                                        DropdownMenuItem(
                                            text = { Text("Remote Commands") },
                                            leadingIcon = {
                                                Icon(
                                                    painterResource(R.drawable.ic_terminal),
                                                    contentDescription = null
                                                )
                                            },
                                            trailingIcon = {
                                                Icon(
                                                    painterResource(R.drawable.ic_next),
                                                    contentDescription = null
                                                )
                                            },
                                            onClick = { showRemoteMenu = true }
                                        )
                                        DropdownMenu(
                                            expanded = showRemoteMenu,
                                            onDismissRequest = { showRemoteMenu = false }
                                        ) {
                                            DropdownMenuItem(
                                                text = { Text("Shutdown") },
                                                onClick = {
                                                    showRemoteMenu = false
                                                    showMenu = false
                                                    app.sendRemoteCommandToDevice(device, RemoteCommand.Shutdown)
                                                }
                                            )
                                            DropdownMenuItem(
                                                text = { Text("Restart") },
                                                onClick = {
                                                    showRemoteMenu = false
                                                    showMenu = false
                                                    app.sendRemoteCommandToDevice(device, RemoteCommand.Restart)
                                                }
                                            )
                                            DropdownMenuItem(
                                                text = { Text("Sign Out") },
                                                onClick = {
                                                    showRemoteMenu = false
                                                    showMenu = false
                                                    app.sendRemoteCommandToDevice(device, RemoteCommand.SignOut)
                                                }
                                            )
                                        }
                                    }
                                }
                                DropdownMenuItem(
                                    text = { Text("Unpair") },
                                    leadingIcon = {
                                        Icon(
                                            painterResource(R.drawable.ic_unpair),
                                            contentDescription = null
                                        )
                                    },
                                    onClick = {
                                        showMenu = false
                                        app.unpairDevice(device)
                                    }
                                )
                            }
                        }
                    }
                } else {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        if (isPending) {
                            Button(
                                onClick = { app.cancelPairing(device) },
                                colors = ButtonDefaults.buttonColors(
                                    containerColor = MaterialTheme.colorScheme.error,
                                    contentColor = MaterialTheme.colorScheme.onError
                                )
                            ) {
                                Text("Cancel")
                            }
                        } else {
                            Button(onClick = {
                                app.setSelectedDeviceForFileTransfer(device)
                                filePickerLauncher?.launch("*/*")
                            }) {
                                Text("Send File")
                            }
                        }

                        Spacer(modifier = Modifier.width(8.dp))

                        if (isPending) {
                            Button(
                                onClick = { },
                                enabled = false
                            ) {
                                Text("Waiting...")
                            }
                        } else {
                            Button(onClick = { app.pairDevice(device) }) {
                                Text("Pair")
                            }
                        }
                    }
                }
            }

            // Media Controls row (below device info) for trusted devices
            if (isTrusted && app.isMediaControlEnabled.value) {
                Row(
                    modifier = Modifier.fillMaxWidth().padding(top = 8.dp),
                    horizontalArrangement = Arrangement.SpaceEvenly,
                    verticalAlignment = Alignment.CenterVertically
                ) {
                    IconButton(onClick = {
                        app.sendMediaCommand(device, uniffi.connected_ffi.MediaCommand.VOLUME_DOWN)
                    }) { Icon(painterResource(R.drawable.ic_volume_down), contentDescription = "Volume Down") }
                    IconButton(onClick = {
                        app.sendMediaCommand(device, uniffi.connected_ffi.MediaCommand.PREVIOUS)
                    }) { Icon(painterResource(R.drawable.ic_previous), contentDescription = "Previous") }
                    IconButton(onClick = {
                        app.sendMediaCommand(device, uniffi.connected_ffi.MediaCommand.PLAY_PAUSE)
                    }) { Icon(painterResource(R.drawable.ic_play), contentDescription = "Play/Pause") }
                    IconButton(onClick = {
                        app.sendMediaCommand(device, uniffi.connected_ffi.MediaCommand.NEXT)
                    }) { Icon(painterResource(R.drawable.ic_next), contentDescription = "Next") }
                    IconButton(onClick = {
                        app.sendMediaCommand(device, uniffi.connected_ffi.MediaCommand.VOLUME_UP)
                    }) { Icon(painterResource(R.drawable.ic_volume_up), contentDescription = "Volume Up") }
                }
            }
        }
    }

    if (showRemoteInput) {
        RemoteInputOverlay(
            device = device,
            app = app,
            onDismiss = { showRemoteInput = false }
        )
    }

    phoneDataKind.value?.let { kind ->
        PhoneDataDialog(
            kind = kind,
            device = device,
            app = app,
            onDismiss = { phoneDataKind.value = null }
        )
    }
}

// ============================================================================
// Remote input overlay (phone -> desktop)
// ============================================================================

private const val REMOTE_BTN_LEFT = 0x110
private const val REMOTE_BTN_RIGHT = 0x111
private const val REMOTE_BTN_MIDDLE = 0x112

private const val KEYSYM_BACKSPACE = 0xFF08
private const val KEYSYM_TAB = 0xFF09
private const val KEYSYM_RETURN = 0xFF0D
private const val KEYSYM_ESCAPE = 0xFF1B
private const val KEYSYM_HOME = 0xFF50
private const val KEYSYM_LEFT = 0xFF51
private const val KEYSYM_UP = 0xFF52
private const val KEYSYM_RIGHT = 0xFF53
private const val KEYSYM_DOWN = 0xFF54
private const val KEYSYM_PAGE_UP = 0xFF55
private const val KEYSYM_PAGE_DOWN = 0xFF56
private const val KEYSYM_END = 0xFF57
private const val KEYSYM_INSERT = 0xFF63
private const val KEYSYM_DELETE = 0xFFFF
private const val KEYSYM_A = 0x61
private const val KEYSYM_SHIFT_L = 0xFFE1
private const val KEYSYM_CONTROL_L = 0xFFE3
private const val KEYSYM_ALT_L = 0xFFE9
private const val KEYSYM_SUPER_L = 0xFFEB

private fun remoteButton(button: Int, pressed: Boolean) =
    RemoteInputEvent.PointerButton(button.toUShort(), pressed)

private fun remoteKey(keysym: Int, pressed: Boolean) =
    RemoteInputEvent.Key(keysym.toUInt(), pressed)

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun RemoteInputOverlay(
    device: DiscoveredDevice,
    app: ConnectedApp,
    onDismiss: () -> Unit
) {
    val context = LocalContext.current
    val view = LocalView.current
    val lifecycleOwner = androidx.lifecycle.compose.LocalLifecycleOwner.current

    var typedText by remember { mutableStateOf("") }
    var ctrlPressed by remember { mutableStateOf(false) }
    var altPressed by remember { mutableStateOf(false) }
    var shiftPressed by remember { mutableStateOf(false) }
    var superPressed by remember { mutableStateOf(false) }
    var dragActive by remember { mutableStateOf(false) }
    var tapDragActive by remember { mutableStateOf(false) }
    var overviewOpenByGesture by remember { mutableStateOf(false) }
    var startError by remember { mutableStateOf<String?>(null) }
    var connected by remember { mutableStateOf(false) }

    fun send(events: List<RemoteInputEvent>) {
        app.sendRemoteControlEvents(device, events)
    }

    fun activeModifiers(): List<Int> = buildList {
        if (ctrlPressed) add(KEYSYM_CONTROL_L)
        if (altPressed) add(KEYSYM_ALT_L)
        if (shiftPressed) add(KEYSYM_SHIFT_L)
        if (superPressed) add(KEYSYM_SUPER_L)
    }

    fun clearModifiers() {
        ctrlPressed = false
        altPressed = false
        shiftPressed = false
        superPressed = false
    }

    fun click(button: Int) {
        send(listOf(remoteButton(button, true), remoteButton(button, false)))
        // A click can move the desktop caret to another field or position.
        // Start a fresh live-typing buffer instead of assuming the old text
        // is still immediately before the caret.
        typedText = ""
        overviewOpenByGesture = false
    }

    fun sendKey(keysym: Int, resetLiveText: Boolean = true) {
        val modifiers = activeModifiers()
        val events = mutableListOf<RemoteInputEvent>()
        modifiers.forEach { events.add(remoteKey(it, true)) }
        events.add(remoteKey(keysym, true))
        events.add(remoteKey(keysym, false))
        modifiers.asReversed().forEach { events.add(remoteKey(it, false)) }
        send(events)
        if (modifiers.isNotEmpty()) clearModifiers()
        if (resetLiveText) typedText = ""
        overviewOpenByGesture = false
    }

    fun sendBackspaces(count: Int) {
        var remaining = count
        while (remaining > 0) {
            // One Backspace press/release pair is two protocol events. Stay
            // below MAX_INPUT_EVENTS_PER_BATCH (256) on every FFI call.
            val chunk = minOf(remaining, 100)
            val events = ArrayList<RemoteInputEvent>(chunk * 2)
            repeat(chunk) {
                events.add(remoteKey(KEYSYM_BACKSPACE, true))
                events.add(remoteKey(KEYSYM_BACKSPACE, false))
            }
            send(events)
            remaining -= chunk
        }
    }

    fun applyTypedText(newValue: String) {
        val previous = typedText
        val modifiers = activeModifiers()

        // A modifier chip plus a single character is a shortcut, not text.
        // Keep the character out of the phone field so it is never echoed.
        // A and C use the same physical key: send the base key, otherwise
        // Ctrl+C would become Ctrl+Shift+C. The Shift chip still applies.
        if (modifiers.isNotEmpty() &&
            newValue.length == previous.length + 1 &&
            newValue.startsWith(previous)
        ) {
            val character = newValue.last()
            val baseCharacter = if (character in 'A'..'Z') character.lowercaseChar() else character
            if (baseCharacter.code in 0x20..0xFF) {
                sendKey(baseCharacter.code)
                return
            }
        }

        val next = limitLiveText(newValue)
        if (next == previous) return

        planRemoteTextEdit(previous, next)?.let { edit ->
            if (edit.backspaces > 0) sendBackspaces(edit.backspaces)
            if (edit.insert.isNotEmpty()) {
                send(listOf(RemoteInputEvent.Text(edit.insert)))
            }
        }
        typedText = next
        if (modifiers.isNotEmpty()) clearModifiers()
        overviewOpenByGesture = false
    }

    fun clearTypedText() {
        sendBackspaces(remoteBackspaceCount(typedText))
        typedText = ""
        overviewOpenByGesture = false
    }

    fun sendChord(modifiers: List<Int>, keysym: Int) {
        val events = mutableListOf<RemoteInputEvent>()
        modifiers.forEach { events.add(remoteKey(it, true)) }
        events.add(remoteKey(keysym, true))
        events.add(remoteKey(keysym, false))
        modifiers.asReversed().forEach { events.add(remoteKey(it, false)) }
        send(events)
    }

    fun zoom(isZoomIn: Boolean) {
        // Ctrl+wheel is the common application zoom shortcut. Keep the press,
        // scroll and release in one ordered batch.
        send(
            listOf(
                remoteKey(KEYSYM_CONTROL_L, true),
                RemoteInputEvent.Scroll(0f, if (isZoomIn) 120f else -120f),
                remoteKey(KEYSYM_CONTROL_L, false)
            )
        )
    }

    fun threeFingerSwipe(dx: Float, dy: Float) {
        if (abs(dx) > abs(dy)) {
            // Natural direction: swiping left moves to the workspace on the
            // right, matching GNOME's three-finger workspace switch.
            val key = if (dx < 0f) KEYSYM_RIGHT else KEYSYM_LEFT
            sendChord(listOf(KEYSYM_CONTROL_L, KEYSYM_ALT_L), key)
        } else if (dy < 0f) {
            if (overviewOpenByGesture) {
                // A second upward swipe opens GNOME's Applications view.
                sendChord(listOf(KEYSYM_SUPER_L), KEYSYM_A)
            } else {
                // GNOME opens the Activities overview on the first swipe up.
                sendChord(emptyList(), KEYSYM_SUPER_L)
                overviewOpenByGesture = true
            }
        } else {
            // GNOME's downward gesture backs out of the overview/app grid.
            sendChord(emptyList(), KEYSYM_ESCAPE)
            overviewOpenByGesture = false
        }
        clearModifiers()
        typedText = ""
    }

    // Start the long-lived ordered stream as soon as the panel opens, so the
    // desktop can present its permission prompt before the first touch.
    DisposableEffect(device.id, device.ip, device.port) {
        val error = app.startRemoteControl(device)
        startError = error
        if (error != null) {
            android.widget.Toast.makeText(
                context,
                "Remote input unavailable: $error",
                android.widget.Toast.LENGTH_LONG
            ).show()
        }
        onDispose {
            // Release a drag before tearing the stream down, otherwise the
            // desktop can be left with a held mouse button.
            if (dragActive || tapDragActive) {
                send(listOf(remoteButton(REMOTE_BTN_LEFT, false)))
            }
            app.stopRemoteControl()
        }
    }

    DisposableEffect(view) {
        view.keepScreenOn = true
        onDispose { view.keepScreenOn = false }
    }

    DisposableEffect(lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_STOP) {
                onDismiss()
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose { lifecycleOwner.lifecycle.removeObserver(observer) }
    }

    LaunchedEffect(device.id, device.ip, device.port) {
        while (true) {
            connected = app.isRemoteControlActive(device)
            delay(400)
        }
    }

    Dialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(usePlatformDefaultWidth = false)
    ) {
        Surface(modifier = Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
            Column(
                modifier = Modifier
                    .fillMaxSize()
                    .safeDrawingPadding()
                    .imePadding()
                    .padding(12.dp)
            ) {
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically
                ) {
                    Icon(
                        painter = painterResource(R.drawable.ic_desktop),
                        contentDescription = null,
                        tint = MaterialTheme.colorScheme.primary
                    )
                    Spacer(modifier = Modifier.width(10.dp))
                    Column(modifier = Modifier.weight(1f)) {
                        Text(
                            text = "Remote Input",
                            style = MaterialTheme.typography.titleLarge
                        )
                        Text(
                            text = device.name,
                            style = MaterialTheme.typography.bodySmall,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis
                        )
                    }
                    IconButton(onClick = onDismiss) {
                        Icon(Icons.Filled.Close, contentDescription = "Close")
                    }
                }

                Text(
                    text = when {
                        startError != null -> "Could not start remote input: $startError"
                        connected -> "Session active. If your desktop is showing a permission prompt, approve it once — it will be remembered."
                        else -> "Waiting for this computer. Enable Remote Input in Settings there; on Wayland, approve the system permission prompt once."
                    },
                    style = MaterialTheme.typography.bodySmall,
                    color = when {
                        startError != null -> MaterialTheme.colorScheme.error
                        connected -> MaterialTheme.colorScheme.primary
                        else -> MaterialTheme.colorScheme.onSurfaceVariant
                    },
                    modifier = Modifier.padding(vertical = 6.dp)
                )

                RemoteTouchpad(
                    modifier = Modifier
                        .weight(1f)
                        .fillMaxWidth()
                        .padding(vertical = 6.dp),
                    onMotion = { dx, dy ->
                        send(
                            listOf(
                                RemoteInputEvent.PointerMotion(dx * 1.35f, dy * 1.35f)
                            )
                        )
                    },
                    onScroll = { dx, dy ->
                        send(listOf(RemoteInputEvent.Scroll(dx, dy)))
                    },
                    onZoom = { zoomIn -> zoom(zoomIn) },
                    onThreeFingerSwipe = { dx, dy -> threeFingerSwipe(dx, dy) },
                    onLeftDown = {
                        if (!dragActive) {
                            tapDragActive = true
                            send(listOf(remoteButton(REMOTE_BTN_LEFT, true)))
                            typedText = ""
                        }
                        overviewOpenByGesture = false
                    },
                    onLeftUp = {
                        if (tapDragActive) {
                            tapDragActive = false
                            send(listOf(remoteButton(REMOTE_BTN_LEFT, false)))
                        }
                    },
                    onClick = { fingers ->
                        click(
                            when (fingers) {
                                1 -> REMOTE_BTN_LEFT
                                2 -> REMOTE_BTN_RIGHT
                                else -> REMOTE_BTN_MIDDLE
                            }
                        )
                    }
                )

                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.spacedBy(6.dp)
                ) {
                    OutlinedButton(
                        onClick = { click(REMOTE_BTN_LEFT) },
                        modifier = Modifier.weight(1f),
                        contentPadding = PaddingValues(horizontal = 8.dp, vertical = 8.dp)
                    ) { Text("Left") }
                    OutlinedButton(
                        onClick = { click(REMOTE_BTN_RIGHT) },
                        modifier = Modifier.weight(1f),
                        contentPadding = PaddingValues(horizontal = 8.dp, vertical = 8.dp)
                    ) { Text("Right") }
                    OutlinedButton(
                        onClick = { click(REMOTE_BTN_MIDDLE) },
                        modifier = Modifier.weight(1f),
                        contentPadding = PaddingValues(horizontal = 8.dp, vertical = 8.dp)
                    ) { Text("Middle") }
                    Button(
                        onClick = {
                            dragActive = !dragActive
                            send(listOf(remoteButton(REMOTE_BTN_LEFT, dragActive)))
                            typedText = ""
                        },
                        modifier = Modifier.weight(1f),
                        contentPadding = PaddingValues(horizontal = 8.dp, vertical = 8.dp),
                        colors = if (dragActive) {
                            ButtonDefaults.buttonColors(
                                containerColor = MaterialTheme.colorScheme.tertiary
                            )
                        } else {
                            ButtonDefaults.buttonColors()
                        }
                    ) { Text(if (dragActive) "Release" else "Drag") }
                }

                Row(
                    modifier = Modifier.fillMaxWidth().padding(top = 6.dp),
                    horizontalArrangement = Arrangement.spacedBy(6.dp)
                ) {
                    OutlinedButton(
                        onClick = { send(listOf(RemoteInputEvent.Scroll(0f, 120f))) },
                        modifier = Modifier.weight(1f),
                        contentPadding = PaddingValues(horizontal = 8.dp, vertical = 6.dp)
                    ) { Text("Scroll Up") }
                    OutlinedButton(
                        onClick = { send(listOf(RemoteInputEvent.Scroll(0f, -120f))) },
                        modifier = Modifier.weight(1f),
                        contentPadding = PaddingValues(horizontal = 8.dp, vertical = 6.dp)
                    ) { Text("Scroll Down") }
                }

                OutlinedTextField(
                    value = typedText,
                    onValueChange = { applyTypedText(it) },
                    modifier = Modifier.fillMaxWidth().padding(top = 8.dp),
                    singleLine = true,
                    label = { Text("Type on desktop") },
                    placeholder = { Text("Sent live as you type") },
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done)
                )

                Row(
                    modifier = Modifier.fillMaxWidth().padding(top = 4.dp),
                    horizontalArrangement = Arrangement.End,
                    verticalAlignment = Alignment.CenterVertically
                ) {
                    OutlinedButton(
                        onClick = { clearTypedText() },
                        enabled = typedText.isNotEmpty(),
                        contentPadding = PaddingValues(horizontal = 12.dp, vertical = 4.dp)
                    ) { Text("Clear") }
                }

                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .horizontalScroll(rememberScrollState())
                        .padding(top = 6.dp),
                    horizontalArrangement = Arrangement.spacedBy(6.dp)
                ) {
                    FilterChip(
                        selected = ctrlPressed,
                        onClick = { ctrlPressed = !ctrlPressed },
                        label = { Text("Ctrl") }
                    )
                    FilterChip(
                        selected = altPressed,
                        onClick = { altPressed = !altPressed },
                        label = { Text("Alt") }
                    )
                    FilterChip(
                        selected = shiftPressed,
                        onClick = { shiftPressed = !shiftPressed },
                        label = { Text("Shift") }
                    )
                    FilterChip(
                        selected = superPressed,
                        onClick = { superPressed = !superPressed },
                        label = { Text("Super") }
                    )
                    RemoteKeyButton("Esc") { sendKey(KEYSYM_ESCAPE) }
                    RemoteKeyButton("Tab") { sendKey(KEYSYM_TAB) }
                    RemoteKeyButton("Enter") { sendKey(KEYSYM_RETURN) }
                    RemoteKeyButton("Bksp") {
                        // Keep the live buffer aligned with a plain Backspace.
                        // A modified Backspace may delete an application-defined
                        // word or line, so drop the buffer instead of guessing.
                        val modified = activeModifiers().isNotEmpty()
                        sendKey(KEYSYM_BACKSPACE, resetLiveText = false)
                        typedText = if (modified) "" else dropLastGrapheme(typedText)
                    }
                    RemoteKeyButton("Del") { sendKey(KEYSYM_DELETE) }
                    RemoteKeyButton("Ins") { sendKey(KEYSYM_INSERT) }
                    RemoteKeyButton("Home") { sendKey(KEYSYM_HOME) }
                    RemoteKeyButton("End") { sendKey(KEYSYM_END) }
                    RemoteKeyButton("PgUp") { sendKey(KEYSYM_PAGE_UP) }
                    RemoteKeyButton("PgDn") { sendKey(KEYSYM_PAGE_DOWN) }
                    RemoteKeyButton("←") { sendKey(KEYSYM_LEFT) }
                    RemoteKeyButton("↑") { sendKey(KEYSYM_UP) }
                    RemoteKeyButton("↓") { sendKey(KEYSYM_DOWN) }
                    RemoteKeyButton("→") { sendKey(KEYSYM_RIGHT) }
                }

                Text(
                    text = "Drag: move · tap: left · two-finger tap: right · three-finger tap: middle\n" +
                            "Two-finger drag: scroll · pinch: zoom · double-tap + drag: select\n" +
                            "Three-finger swipe: ↑ Activities · ↑ again Apps · ↓ Back · ←/→ Workspaces",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(top = 6.dp)
                )
            }
        }
    }
}

@Composable
private fun RemoteKeyButton(label: String, onClick: () -> Unit) {
    OutlinedButton(
        onClick = onClick,
        contentPadding = PaddingValues(horizontal = 10.dp, vertical = 4.dp)
    ) {
        Text(label)
    }
}

private enum class TwoFingerMode { Undecided, Scroll, Zoom }

@Composable
private fun RemoteTouchpad(
    modifier: Modifier = Modifier,
    onMotion: (Float, Float) -> Unit,
    onScroll: (Float, Float) -> Unit,
    onZoom: (Boolean) -> Unit,
    onThreeFingerSwipe: (Float, Float) -> Unit,
    onLeftDown: () -> Unit,
    onLeftUp: () -> Unit,
    onClick: (Int) -> Unit
) {
    // The pointerInput block is long-lived. Keep these callbacks current so a
    // stale composition cannot leave a mouse button held or ignore a chip.
    val currentOnMotion by rememberUpdatedState(onMotion)
    val currentOnScroll by rememberUpdatedState(onScroll)
    val currentOnZoom by rememberUpdatedState(onZoom)
    val currentOnSwipe by rememberUpdatedState(onThreeFingerSwipe)
    val currentOnLeftDown by rememberUpdatedState(onLeftDown)
    val currentOnLeftUp by rememberUpdatedState(onLeftUp)
    val currentOnClick by rememberUpdatedState(onClick)

    Box(
        modifier = modifier
            .clip(RoundedCornerShape(16.dp))
            .background(MaterialTheme.colorScheme.surfaceVariant)
            .pointerInput(Unit) {
                val swipeThreshold = 72.dp.toPx()
                val threeFingerTapSlop = 24.dp.toPx()
                val doubleTapSlop = 36.dp.toPx()
                val zoomStep = 48.dp.toPx()
                var lastTapUpTime = 0L
                var lastTapPosition = Offset.Zero

                awaitEachGesture {
                    val down = awaitFirstDown(requireUnconsumed = false)
                    val doubleTap = lastTapUpTime != 0L &&
                            down.uptimeMillis - lastTapUpTime in 0L..viewConfiguration.doubleTapTimeoutMillis &&
                            (down.position - lastTapPosition).getDistance() <= doubleTapSlop
                    if (doubleTap) lastTapUpTime = 0L

                    var lastCentroid = down.position
                    var lastPointerCount = 1
                    var maxPointerCount = 1
                    var moved = false
                    var motionAccum = Offset.Zero
                    var threeFingerAccum = Offset.Zero
                    var twoFingerMode = TwoFingerMode.Undecided
                    var centroidAccum = Offset.Zero
                    var spreadAccum = 0f
                    var lastPinchDistance = 0f
                    var zoomAccum = 0f
                    var tapDragCandidate = doubleTap
                    var dragPressSent = false
                    var tapDragging = false

                    try {
                        while (true) {
                            val event = awaitPointerEvent(PointerEventPass.Main)
                            val pressed = event.changes.filter { it.pressed }
                            if (pressed.isEmpty()) break

                            maxPointerCount = maxOf(maxPointerCount, pressed.size)
                            val centroid = pressed.fold(Offset.Zero) { acc, change ->
                                acc + change.position
                            } / pressed.size.toFloat()

                            if (pressed.size != lastPointerCount) {
                                // Adding or lifting a finger moves the centroid;
                                // don't turn that jump into input.
                                lastPointerCount = pressed.size
                                lastCentroid = centroid
                                if (pressed.size == 2) {
                                    lastPinchDistance =
                                        (pressed[0].position - pressed[1].position).getDistance()
                                    twoFingerMode = TwoFingerMode.Undecided
                                    centroidAccum = Offset.Zero
                                    spreadAccum = 0f
                                    zoomAccum = 0f
                                }
                                if (tapDragging) {
                                    tapDragging = false
                                    currentOnLeftUp()
                                }
                                event.changes.forEach { it.consume() }
                                continue
                            }

                            val delta = centroid - lastCentroid
                            lastCentroid = centroid

                            if (maxPointerCount >= 3) {
                                // Three- and four-finger gestures are reserved
                                // for GNOME navigation; never emit pointer or
                                // scroll input for them.
                                if (pressed.size >= 3) {
                                    threeFingerAccum += delta
                                    if (threeFingerAccum.getDistance() > threeFingerTapSlop) {
                                        moved = true
                                    }
                                }
                            } else if (pressed.size == 2) {
                                val pinchDistance =
                                    (pressed[0].position - pressed[1].position).getDistance()
                                val spreadDelta = pinchDistance - lastPinchDistance
                                lastPinchDistance = pinchDistance
                                centroidAccum += delta
                                spreadAccum += spreadDelta

                                when (twoFingerMode) {
                                    TwoFingerMode.Undecided -> {
                                        val centroidDistance = centroidAccum.getDistance()
                                        val spreadDistance = abs(spreadAccum)
                                        // A pinch needs a clear spread change;
                                        // otherwise a slightly uneven two-finger
                                        // scroll would get mistaken for zoom.
                                        val zoomIntent = spreadDistance >
                                                maxOf(
                                                    centroidDistance * 1.5f,
                                                    viewConfiguration.touchSlop * 2f
                                                )
                                        if (zoomIntent ||
                                            centroidDistance > viewConfiguration.touchSlop
                                        ) {
                                            moved = true
                                            twoFingerMode =
                                                if (zoomIntent) {
                                                    TwoFingerMode.Zoom
                                                } else {
                                                    TwoFingerMode.Scroll
                                                }
                                            if (twoFingerMode == TwoFingerMode.Scroll) {
                                                currentOnScroll(
                                                    centroidAccum.x,
                                                    centroidAccum.y
                                                )
                                                centroidAccum = Offset.Zero
                                            } else {
                                                zoomAccum = spreadAccum
                                                spreadAccum = 0f
                                                while (abs(zoomAccum) >= zoomStep) {
                                                    currentOnZoom(zoomAccum > 0f)
                                                    zoomAccum -= if (zoomAccum > 0f) {
                                                        zoomStep
                                                    } else {
                                                        -zoomStep
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    TwoFingerMode.Scroll -> {
                                        if (delta != Offset.Zero) {
                                            currentOnScroll(delta.x, delta.y)
                                        }
                                    }

                                    TwoFingerMode.Zoom -> {
                                        zoomAccum += spreadDelta
                                        while (abs(zoomAccum) >= zoomStep) {
                                            currentOnZoom(zoomAccum > 0f)
                                            zoomAccum -= if (zoomAccum > 0f) {
                                                zoomStep
                                            } else {
                                                -zoomStep
                                            }
                                        }
                                    }
                                }
                            } else if (maxPointerCount == 1) {
                                if (!moved) {
                                    motionAccum += delta
                                    if (motionAccum.getDistance() >
                                        viewConfiguration.touchSlop
                                    ) {
                                        moved = true
                                        if (tapDragCandidate) {
                                            tapDragCandidate = false
                                            dragPressSent = true
                                            tapDragging = true
                                            currentOnLeftDown()
                                        }
                                        currentOnMotion(motionAccum.x, motionAccum.y)
                                        motionAccum = Offset.Zero
                                    }
                                } else if (delta != Offset.Zero) {
                                    currentOnMotion(delta.x, delta.y)
                                }
                            }
                            event.changes.forEach { it.consume() }
                        }
                    } finally {
                        if (tapDragging) {
                            tapDragging = false
                            currentOnLeftUp()
                        }
                    }

                    if (!moved && !dragPressSent) {
                        currentOnClick(maxPointerCount)
                        if (maxPointerCount == 1 && !doubleTap) {
                            lastTapUpTime = SystemClock.uptimeMillis()
                            lastTapPosition = down.position
                        }
                    }

                    if (maxPointerCount >= 3 &&
                        moved &&
                        threeFingerAccum.getDistance() >= swipeThreshold
                    ) {
                        currentOnSwipe(threeFingerAccum.x, threeFingerAccum.y)
                    }
                }
            },
        contentAlignment = Alignment.Center
    ) {
        Text(
            text = "Touchpad",
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant
        )
    }
}
