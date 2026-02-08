package com.connected.app

import android.Manifest
import android.annotation.SuppressLint
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.os.Build
import android.content.pm.PackageManager
import android.provider.Settings
import android.text.TextUtils
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.contract.ActivityResultContracts
import androidx.annotation.RequiresApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
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
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Warning
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.nestedscroll.NestedScrollConnection
import androidx.compose.ui.input.nestedscroll.NestedScrollSource
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.core.content.ContextCompat
import androidx.core.os.BundleCompat
import kotlinx.coroutines.launch
import uniffi.connected_ffi.DiscoveredDevice
import androidx.core.net.toUri

@OptIn(ExperimentalMaterial3Api::class)
class MainActivity : ComponentActivity() {
    private lateinit var connectedApp: ConnectedApp
    private lateinit var proximityPermissionsLauncher: ActivityResultLauncher<Array<String>>
    private var proximityPermissionsInFlight = false

    private val filePickerLauncher = registerForActivityResult(ActivityResultContracts.GetContent()) { uri ->
        uri?.let { selectedUri ->
            connectedApp.getSelectedDeviceForFileTransfer()?.let { device ->
                connectedApp.sendFileToDevice(device, selectedUri.toString())
            }
        }
    }

    private val folderPickerLauncher = registerForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri ->
        uri?.let {
            connectedApp.setRootUri(it)
        }
    }

    private val sendFolderLauncher = registerForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri ->
        uri?.let { selectedUri ->
            connectedApp.getSelectedDeviceForFileTransfer()?.let { device ->
                connectedApp.sendFolderToDevice(device, selectedUri)
            }
        }
    }

    @RequiresApi(Build.VERSION_CODES.R)
    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)

        // Initialize singleton with Application Context
        connectedApp = ConnectedApp.getInstance(applicationContext)

        // If service is running, it has already initialized the app logic.
        if (!ConnectedService.isRunning) {
            connectedApp.initialize()
        }

        proximityPermissionsLauncher = registerForActivityResult(
            ActivityResultContracts.RequestMultiplePermissions()
        ) {
            proximityPermissionsInFlight = false
            connectedApp.startProximityManager()
        }

        requestProximityPermissionsIfNeeded()
        handleShareIntent(intent)

        setContent {
            ConnectedTheme {
                Surface(modifier = Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
                    if (connectedApp.isBrowsingRemote.value) {
                        RemoteFileBrowser(connectedApp)
                    } else {
                        MainAppNavigation(
                            connectedApp,
                            filePickerLauncher,
                            folderPickerLauncher,
                            sendFolderLauncher
                        )
                    }
                }
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handleShareIntent(intent)
    }

    override fun onDestroy() {
        // Only cleanup if we are NOT running as a service
        if (!ConnectedService.isRunning) {
            connectedApp.cleanup()
        }
        super.onDestroy()
    }

    override fun onResume() {
        super.onResume()
        connectedApp.setAppInForeground(true)
        connectedApp.resumePendingUpdateInstallIfAllowed()
        requestProximityPermissionsIfNeeded()
    }

    override fun onPause() {
        connectedApp.setAppInForeground(false)
        super.onPause()
    }

    private fun requestProximityPermissionsIfNeeded() {
        val missing = LinkedHashSet<String>()

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            if (ContextCompat.checkSelfPermission(this, Manifest.permission.BLUETOOTH_SCAN) !=
                PackageManager.PERMISSION_GRANTED
            ) {
                missing.add(Manifest.permission.BLUETOOTH_SCAN)
            }
            if (ContextCompat.checkSelfPermission(this, Manifest.permission.BLUETOOTH_ADVERTISE) !=
                PackageManager.PERMISSION_GRANTED
            ) {
                missing.add(Manifest.permission.BLUETOOTH_ADVERTISE)
            }
            if (ContextCompat.checkSelfPermission(this, Manifest.permission.BLUETOOTH_CONNECT) !=
                PackageManager.PERMISSION_GRANTED
            ) {
                missing.add(Manifest.permission.BLUETOOTH_CONNECT)
            }
        } else {
            if (ContextCompat.checkSelfPermission(this, Manifest.permission.BLUETOOTH) !=
                PackageManager.PERMISSION_GRANTED
            ) {
                missing.add(Manifest.permission.BLUETOOTH)
            }
        }

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
        }

        if (missing.isEmpty()) {
            connectedApp.startProximityManager()
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
            Intent.ACTION_SEND -> {
                val uri = intent.extras?.let { BundleCompat.getParcelable(it, Intent.EXTRA_STREAM, Uri::class.java) }
                    ?: intent.clipData?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.uri
                if (uri != null) {
                    connectedApp.setPendingShare(listOf(uri))
                }
            }

            Intent.ACTION_SEND_MULTIPLE -> {
                val uris =
                    intent.extras?.let { BundleCompat.getParcelableArrayList(it, Intent.EXTRA_STREAM, Uri::class.java) }
                        ?: intent.clipData?.let { clip ->
                            ArrayList<Uri>(clip.itemCount).apply {
                                for (i in 0 until clip.itemCount) {
                                    clip.getItemAt(i)?.uri?.let { add(it) }
                                }
                            }
                        }
                if (!uris.isNullOrEmpty()) {
                    connectedApp.setPendingShare(uris)
                }
            }
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
@RequiresApi(Build.VERSION_CODES.R)
fun openNotificationListenerSettings(context: Context) {
    try {
        val intent = Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS)
        intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        context.startActivity(intent)
    } catch (_: Exception) {
        // Fallback for devices that don't support the direct intent
        try {
            val fallbackIntent = Intent(Settings.ACTION_NOTIFICATION_LISTENER_DETAIL_SETTINGS)
            fallbackIntent.putExtra(
                Settings.EXTRA_NOTIFICATION_LISTENER_COMPONENT_NAME,
                android.content.ComponentName(context, MediaObserverService::class.java).flattenToString()
            )
            fallbackIntent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            context.startActivity(fallbackIntent)
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
    if (kb < 1024.0) return String.format("%.1f KB", kb)
    val mb = kb / 1024.0
    if (mb < 1024.0) return String.format("%.1f MB", mb)
    val gb = mb / 1024.0
    return String.format("%.1f GB", gb)
}

fun getDeviceIcon(type: String): Int {
    return when (type.lowercase()) {
        "android", "phone", "mobile" -> R.drawable.ic_android
        "ios", "iphone" -> R.drawable.ic_ios
        "macos", "mac" -> R.drawable.ic_macos
        "windows", "pc" -> R.drawable.ic_windows
        "linux" -> R.drawable.ic_linux
        "tablet", "ipad" -> R.drawable.ic_tablet
        "desktop" -> R.drawable.ic_desktop
        "laptop" -> R.drawable.ic_laptop
        "tv" -> R.drawable.ic_tv
        "watch" -> R.drawable.ic_watch
        else -> R.drawable.ic_device_unknown
    }
}

enum class Screen {
    Home,
    Settings
}

@RequiresApi(Build.VERSION_CODES.R)
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MainAppNavigation(
    connectedApp: ConnectedApp,
    filePickerLauncher: ActivityResultLauncher<String>? = null,
    folderPickerLauncher: ActivityResultLauncher<Uri?>? = null,
    sendFolderLauncher: ActivityResultLauncher<Uri?>? = null
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
            NavigationBar {
                NavigationBarItem(
                    icon = { Icon(painterResource(R.drawable.ic_nav_devices), contentDescription = "Devices") },
                    label = { Text("Devices") },
                    selected = currentScreen == Screen.Home,
                    onClick = { currentScreen = Screen.Home },
                    colors = NavigationBarItemDefaults.colors(
                        selectedIconColor = androidx.compose.ui.graphics.Color.White,
                        selectedTextColor = androidx.compose.ui.graphics.Color.White,
                        indicatorColor = androidx.compose.ui.graphics.Color(0xFF333333),
                        unselectedIconColor = androidx.compose.ui.graphics.Color.Gray,
                        unselectedTextColor = androidx.compose.ui.graphics.Color.Gray
                    )
                )
                NavigationBarItem(
                    icon = { Icon(painterResource(R.drawable.ic_nav_settings), contentDescription = "Settings") },
                    label = { Text("Settings") },
                    selected = currentScreen == Screen.Settings,
                    onClick = {
                        @Suppress("AssignedValueIsNeverRead")
                        currentScreen = Screen.Settings
                    },
                    colors = NavigationBarItemDefaults.colors(
                        selectedIconColor = androidx.compose.ui.graphics.Color.White,
                        selectedTextColor = androidx.compose.ui.graphics.Color.White,
                        indicatorColor = androidx.compose.ui.graphics.Color(0xFF333333),
                        unselectedIconColor = androidx.compose.ui.graphics.Color.Gray,
                        unselectedTextColor = androidx.compose.ui.graphics.Color.Gray
                    )
                )
            }
        }
    ) { paddingValues ->
        Box(modifier = Modifier.padding(paddingValues)) {
            when (currentScreen) {
                Screen.Home -> HomeScreen(connectedApp, filePickerLauncher, sendFolderLauncher)
                Screen.Settings -> SettingsScreen(connectedApp, folderPickerLauncher)
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
                                            painterResource(getDeviceIcon(device.deviceType)),
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
            AlertDialog(
                onDismissRequest = { connectedApp.rejectTransfer(request) },
                title = { Text("Incoming File") },
                text = { Text("${request.fromDevice} wants to send:\n${request.filename}\nSize: ${request.fileSize} bytes") },
                confirmButton = {
                    Button(onClick = { connectedApp.acceptTransfer(request) }) {
                        Text("Accept")
                    }
                },
                dismissButton = {
                    Button(onClick = { connectedApp.rejectTransfer(request) }) {
                        Text("Reject")
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

    val nestedScrollConnection = remember {
        object : NestedScrollConnection {
            override fun onPreScroll(available: Offset, source: NestedScrollSource): Offset {
                // When scrolling back up while pulled down, consume the scroll to reduce pullOffset
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
                // When at top of list and user pulls down, accumulate offset
                if (available.y > 0f && source == NestedScrollSource.UserInput) {
                    pullOffset = (pullOffset + available.y * 0.5f).coerceAtMost(refreshThreshold * 1.5f)
                    return Offset(0f, available.y)
                }
                return Offset.Zero
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
                // Pull-to-refresh indicator
                if (pullOffset > 0f || isRefreshing) {
                    Box(
                        modifier = Modifier.fillMaxWidth()
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

                // Release detection
                LaunchedEffect(pullOffset) {
                    if (pullOffset <= 0f && !isRefreshing) return@LaunchedEffect
                    // Use a small delay to detect when the user lifts their finger
                    kotlinx.coroutines.delay(100)
                    if (pullOffset >= refreshThreshold && !isRefreshing) {
                        isRefreshing = true
                        pullOffset = 0f
                        connectedApp.refreshDeviceDiscovery()
                        kotlinx.coroutines.delay(1500)
                        isRefreshing = false
                    } else if (!isRefreshing) {
                        pullOffset = 0f
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
                        overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis
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
                Text(
                    connectedApp.transferStatus.value,
                    style = MaterialTheme.typography.bodySmall,
                    modifier = Modifier.padding(12.dp)
                )
            }
        }
    }
}

private fun formatBytes(bytes: Long): String {
    return when {
        bytes >= 1_073_741_824 -> String.format("%.1f GB", bytes / 1_073_741_824.0)
        bytes >= 1_048_576 -> String.format("%.1f MB", bytes / 1_048_576.0)
        bytes >= 1024 -> String.format("%.1f KB", bytes / 1024.0)
        else -> "$bytes B"
    }
}

private fun formatDuration(seconds: Long): String {
    return when {
        seconds >= 3600 -> String.format("%d:%02d:%02d", seconds / 3600, (seconds % 3600) / 60, seconds % 60)
        seconds >= 60 -> String.format("%d:%02d", seconds / 60, seconds % 60)
        else -> "${seconds}s"
    }
}

@RequiresApi(Build.VERSION_CODES.R)
@SuppressLint("BatteryLife")
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsScreen(
    connectedApp: ConnectedApp,
    folderPickerLauncher: ActivityResultLauncher<Uri?>? = null
) {
    val context = LocalContext.current
    val lifecycleOwner = androidx.lifecycle.compose.LocalLifecycleOwner.current
    var isNotificationAccessGranted by remember { mutableStateOf(false) }
    var isBackgroundServiceRunning by remember {
        mutableStateOf(ConnectedService.isRunning)
    }

    // Battery Optimization
    val powerManager = context.getSystemService(Context.POWER_SERVICE) as android.os.PowerManager
    var isIgnoringBatteryOptimizations by remember {
        mutableStateOf(
            powerManager.isIgnoringBatteryOptimizations(context.packageName)
        )
    }

    // Telephony permissions
    var hasTelephonyPermissions by remember { mutableStateOf(false) }
    var permissionsRequested by remember { mutableStateOf(false) }

    DisposableEffect(lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) {
                // Check background service
                isBackgroundServiceRunning = ConnectedService.isRunning

                // Check battery optimizations
                isIgnoringBatteryOptimizations = powerManager.isIgnoringBatteryOptimizations(context.packageName)

                // Check notification access using robust method
                isNotificationAccessGranted = isNotificationListenerEnabled(context, MediaObserverService::class.java)

                // Check telephony permissions
                hasTelephonyPermissions = connectedApp.telephonyProvider.hasContactsPermission() &&
                        connectedApp.telephonyProvider.hasSmsPermission() &&
                        connectedApp.telephonyProvider.hasCallLogPermission() &&
                        connectedApp.telephonyProvider.hasPhonePermission() &&
                        connectedApp.telephonyProvider.hasAnswerPhoneCallsPermission()
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose {
            lifecycleOwner.lifecycle.removeObserver(observer)
        }
    }
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

    fun openAppPermissionSettings() {
        val intent = Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS).apply {
            data = Uri.fromParts("package", context.packageName, null)
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        }
        context.startActivity(intent)
    }

    val telephonyPermissionLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions()
    ) { permissions ->
        permissionsRequested = true
        hasTelephonyPermissions = permissions.values.all { it }
        if (hasTelephonyPermissions && !connectedApp.isTelephonyEnabled.value) {
            connectedApp.toggleTelephony()
        }
    }

    var showRenameDialog by remember { mutableStateOf(false) }

    if (showRenameDialog) {
        var newName by remember { mutableStateOf(connectedApp.getDeviceName()) }
        AlertDialog(
            onDismissRequest = {
                @Suppress("AssignedValueIsNeverRead")
                showRenameDialog = false
            },
            title = { Text("Rename Device") },
            text = {
                OutlinedTextField(
                    value = newName,
                    onValueChange = {
                        @Suppress("AssignedValueIsNeverRead")
                        newName = it
                    },
                    label = { Text("Device Name") },
                    singleLine = true
                )
            },
            confirmButton = {
                Button(onClick = {
                    if (newName.isNotBlank()) {
                        connectedApp.renameDevice(newName)
                        @Suppress("AssignedValueIsNeverRead")
                        showRenameDialog = false
                    }
                }) {
                    Text("Save")
                }
            },
            dismissButton = {
                Button(onClick = {
                    @Suppress("AssignedValueIsNeverRead")
                    showRenameDialog = false
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
                            connectedApp.getDeviceName(),
                            style = MaterialTheme.typography.bodyLarge,
                            modifier = Modifier.padding(start = 4.dp)
                        )
                        Button(onClick = {
                            @Suppress("AssignedValueIsNeverRead")
                            showRenameDialog = true
                        }) {
                            Text("Rename")
                        }
                    }
                }
            }
        }

        // Run in Background Section
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
                            checked = isBackgroundServiceRunning,
                            onCheckedChange = { enabled ->
                                val intent = Intent(context, ConnectedService::class.java)
                                if (enabled) {
                                    context.startForegroundService(intent)
                                } else {
                                    context.stopService(intent)
                                }
                                isBackgroundServiceRunning = enabled
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

        // Phone Link / Telephony Section
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
                                "SMS, calls, and contacts sync",
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant
                            )
                        }
                        Switch(
                            checked = connectedApp.isTelephonyEnabled.value,
                            onCheckedChange = { enabled ->
                                if (enabled && !hasTelephonyPermissions) {
                                    telephonyPermissionLauncher.launch(
                                        connectedApp.telephonyProvider.getRequiredPermissions()
                                    )
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
                        PermissionStatusRow(
                            label = "Phone",
                            granted = connectedApp.telephonyProvider.hasPhonePermission()
                        )
                        PermissionStatusRow(
                            label = "Answer Calls",
                            granted = connectedApp.telephonyProvider.hasAnswerPhoneCallsPermission()
                        )
                    }

                    if (!hasTelephonyPermissions) {
                        Spacer(modifier = Modifier.height(12.dp))
                        Button(
                            onClick = {
                                if (shouldOpenSettings()) {
                                    openAppPermissionSettings()
                                } else {
                                    telephonyPermissionLauncher.launch(
                                        connectedApp.telephonyProvider.getRequiredPermissions()
                                    )
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

                    if (connectedApp.isTelephonyEnabled.value && !isNotificationAccessGranted) {
                        Spacer(modifier = Modifier.height(12.dp))
                        Text(
                            "RCS preview requires Notification Access (Google Messages).",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant
                        )
                        Spacer(modifier = Modifier.height(8.dp))
                        Button(
                            onClick = {
                                openNotificationListenerSettings(context)
                            },
                            colors = ButtonDefaults.buttonColors(
                                containerColor = MaterialTheme.colorScheme.secondaryContainer,
                                contentColor = MaterialTheme.colorScheme.onSecondaryContainer
                            ),
                            modifier = Modifier.fillMaxWidth()
                        ) {
                            Text("Enable Notification Access")
                        }
                    }
                }
            }
        }

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
                        Text("Select Specific Folder")
                    }
                }
            }
        }

        // App Update Section
        item {
            Card(
                colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant),
                modifier = Modifier.fillMaxWidth()
            ) {
                Column(modifier = Modifier.padding(16.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Icon(
                            painterResource(R.drawable.ic_download),
                            contentDescription = null,
                            tint = MaterialTheme.colorScheme.primary,
                            modifier = Modifier.size(24.dp)
                        )
                        Spacer(modifier = Modifier.width(16.dp))
                        Text("App Updates", style = MaterialTheme.typography.titleMedium)
                    }
                    Spacer(modifier = Modifier.height(12.dp))

                    var checkingForUpdate by remember { mutableStateOf(false) }
                    var updateInfo by remember { mutableStateOf<uniffi.connected_ffi.UpdateInfo?>(null) }
                    var isDownloading by remember { mutableStateOf(false) }
                    var downloadProgress by remember { mutableStateOf(0) }
                    val scope = rememberCoroutineScope()

                    if (updateInfo == null) {
                        Button(
                            onClick = {
                                checkingForUpdate = true
                                scope.launch {
                                    updateInfo = connectedApp.checkForUpdates()
                                    checkingForUpdate = false
                                    if (updateInfo?.hasUpdate == false) {
                                        android.widget.Toast.makeText(
                                            context,
                                            "No updates available",
                                            android.widget.Toast.LENGTH_SHORT
                                        ).show()
                                    }
                                }
                            },
                            enabled = !checkingForUpdate,
                            modifier = Modifier.fillMaxWidth()
                        ) {
                            if (checkingForUpdate) {
                                CircularProgressIndicator(
                                    modifier = Modifier.size(16.dp),
                                    color = MaterialTheme.colorScheme.onPrimary,
                                    strokeWidth = 2.dp
                                )
                                Spacer(modifier = Modifier.width(8.dp))
                                Text("Checking...")
                            } else {
                                Text("Check for Updates")
                            }
                        }
                    } else if (updateInfo!!.hasUpdate) {
                        Text(
                            "New version available: ${updateInfo!!.latestVersion}",
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.primary
                        )
                        if (updateInfo!!.releaseNotes != null) {
                            Text(
                                updateInfo!!.releaseNotes!!,
                                style = MaterialTheme.typography.bodySmall,
                                modifier = Modifier.padding(vertical = 8.dp)
                            )
                        }

                        if (isDownloading) {
                            LinearProgressIndicator(
                                progress = { downloadProgress / 100f },
                                modifier = Modifier.fillMaxWidth().height(8.dp),
                            )
                            Text(
                                "Downloading: $downloadProgress%",
                                style = MaterialTheme.typography.bodySmall,
                                modifier = Modifier.align(Alignment.End)
                            )
                        } else {
                            Button(
                                onClick = {
                                    val url = updateInfo!!.downloadUrl
                                    if (url != null) {
                                        isDownloading = true
                                        connectedApp.downloadUpdate(url, { progress ->
                                            downloadProgress = progress
                                        }, { file ->
                                            isDownloading = false
                                            if (file != null) {
                                                connectedApp.installApk(file)
                                            } else {
                                                android.widget.Toast.makeText(
                                                    context,
                                                    "Download failed",
                                                    android.widget.Toast.LENGTH_SHORT
                                                ).show()
                                            }
                                        })
                                    } else {
                                        android.widget.Toast.makeText(
                                            context,
                                            "No download URL found",
                                            android.widget.Toast.LENGTH_SHORT
                                        ).show()
                                    }
                                },
                                modifier = Modifier.fillMaxWidth()
                            ) {
                                Text("Download & Install")
                            }
                        }
                    } else {
                        Text("You are on the latest version.")
                        Spacer(modifier = Modifier.height(8.dp))
                        Button(
                            onClick = { updateInfo = null },
                            modifier = Modifier.fillMaxWidth()
                        ) {
                            Text("Check Again")
                        }
                    }
                }
            }
        }

        // Troubleshooting Section
        if (!isNotificationAccessGranted) {
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

    Card(modifier = Modifier.padding(vertical = 4.dp).fillMaxWidth()) {
        Column(modifier = Modifier.padding(12.dp).fillMaxWidth()) {
            // Top row: Device info and action buttons
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically
            ) {
                Row(modifier = Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically) {
                    Column(modifier = Modifier.clickable { showDetails = !showDetails }) {
                        Text(
                            text = device.name,
                            style = MaterialTheme.typography.bodyLarge,
                            maxLines = 1,
                            overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis
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
                                DropdownMenuItem(
                                    text = { Text("Forget") },
                                    leadingIcon = {
                                        Icon(
                                            painterResource(R.drawable.ic_refresh),
                                            contentDescription = null
                                        )
                                    },
                                    onClick = {
                                        showMenu = false
                                        app.forgetDevice(device)
                                    }
                                )
                            }
                        }
                    }
                } else {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Button(onClick = {
                            app.setSelectedDeviceForFileTransfer(device)
                            filePickerLauncher?.launch("*/*")
                        }) {
                            Text("Send File")
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
}
