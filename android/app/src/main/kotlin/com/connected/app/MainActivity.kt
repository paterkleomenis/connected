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
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
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
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
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

    override fun onCreate(savedInstanceState: Bundle?) {
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
            connectedApp.startProximity()
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
            connectedApp.startProximity()
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
                val uris = intent.extras?.let { BundleCompat.getParcelableArrayList(it, Intent.EXTRA_STREAM, Uri::class.java) }
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

@Composable
fun RemoteFileBrowser(app: ConnectedApp) {
    Column(modifier = Modifier.fillMaxSize().padding(16.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = { app.closeRemoteBrowser() }) {
                Icon(painterResource(R.drawable.ic_back), contentDescription = "Back")
            }
            Text("Remote Files: ${app.currentRemotePath.value}", style = MaterialTheme.typography.titleMedium)
        }

        LazyColumn(modifier = Modifier.padding(top = 8.dp)) {
            if (app.currentRemotePath.value != "/") {
                item {
                    Card(
                        modifier = Modifier.padding(vertical = 4.dp).fillMaxWidth(),
                        onClick = {
                            val current = app.currentRemotePath.value
                            val parent = current.substringBeforeLast('/').ifEmpty { "/" }
                            app.browseRemoteFiles(app.getBrowsingDevice()!!, parent)
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

                Card(
                    modifier = Modifier.padding(vertical = 4.dp).fillMaxWidth(),
                    onClick = {
                        if (file.entryType == uniffi.connected_ffi.FfiFsEntryType.DIRECTORY) {
                            app.browseRemoteFiles(app.getBrowsingDevice()!!, file.path)
                        } else {
                            app.getBrowsingDevice()?.let { device ->
                                app.downloadRemoteFile(device, file.path)
                            }
                        }
                    }
                ) {
                    Row(
                        modifier = Modifier.padding(16.dp).fillMaxWidth(),
                        horizontalArrangement = Arrangement.SpaceBetween,
                        verticalAlignment = Alignment.CenterVertically
                    ) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
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

                            Text(file.name)
                        }
                        Text(if (file.entryType == uniffi.connected_ffi.FfiFsEntryType.DIRECTORY) "" else "${file.size} B")
                    }
                }
            }
        }
    }
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
                    onClick = { currentScreen = Screen.Home }
                )
                NavigationBarItem(
                    icon = { Icon(painterResource(R.drawable.ic_nav_settings), contentDescription = "Settings") },
                    label = { Text("Settings") },
                    selected = currentScreen == Screen.Settings,
                    onClick = {
                        @Suppress("AssignedValueIsNeverRead")
                        currentScreen = Screen.Settings
                    }
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

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun HomeScreen(
    connectedApp: ConnectedApp,
    filePickerLauncher: ActivityResultLauncher<String>? = null,
    sendFolderLauncher: ActivityResultLauncher<Uri?>? = null
) {
    val context = LocalContext.current
    var areNotificationsEnabled by remember { mutableStateOf(true) }

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
            LazyColumn(modifier = Modifier.weight(1f)) {
                items(connectedApp.devices) { device ->
                    DeviceItem(device, connectedApp, filePickerLauncher, sendFolderLauncher)
                }
            }
        }

        // Transfer status at bottom
        if (connectedApp.transferStatus.value.isNotEmpty() && connectedApp.transferStatus.value != "Idle") {
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

                // Check notification access
                val componentName = android.content.ComponentName(context, MediaObserverService::class.java)
                val enabledListeners = Settings.Secure.getString(
                    context.contentResolver,
                    "enabled_notification_listeners"
                )
                isNotificationAccessGranted =
                    enabledListeners != null && enabledListeners.contains(componentName.flattenToString())

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
                                val intent =
                                    Intent("android.settings.ACTION_NOTIFICATION_LISTENER_SETTINGS")
                                context.startActivity(intent)
                            },
                            colors = ButtonDefaults.buttonColors(
                                containerColor = MaterialTheme.colorScheme.errorContainer,
                                contentColor = MaterialTheme.colorScheme.onErrorContainer
                            ),
                            modifier = Modifier.fillMaxWidth()
                        ) {
                            Text("⚠️ Grant Notification Access")
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
                                val intent =
                                    Intent("android.settings.ACTION_NOTIFICATION_LISTENER_SETTINGS")
                                context.startActivity(intent)
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

    Card(modifier = Modifier.padding(vertical = 4.dp).fillMaxWidth()) {
        Row(
            modifier = Modifier.padding(12.dp).fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically
        ) {
            Row(modifier = Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically) {
                // Device Type Icon
                Box(
                    modifier = Modifier
                        .size(40.dp)
                        .padding(end = 8.dp),
                    contentAlignment = Alignment.Center
                ) {
                    Icon(
                        painterResource(getDeviceIcon(device.deviceType)),
                        contentDescription = device.deviceType,
                        modifier = Modifier.size(24.dp),
                        tint = MaterialTheme.colorScheme.primary
                    )
                }

                Column {
                    Text(text = device.name, style = MaterialTheme.typography.bodyLarge)
                    Text(text = "${device.ip}:${device.port}", style = MaterialTheme.typography.bodySmall)
                    if (isTrusted) {
                        Text(
                            text = "✓ Trusted",
                            style = MaterialTheme.typography.labelSmall,
                            color = MaterialTheme.colorScheme.primary
                        )
                    }
                }
            }

            if (isTrusted) {
                Row {
                    // Media Controls (if enabled)
                    if (app.isMediaControlEnabled.value) {
                        IconButton(onClick = {
                            app.sendMediaCommand(device, uniffi.connected_ffi.MediaCommand.PREVIOUS)
                        }) { Icon(painterResource(R.drawable.ic_previous), contentDescription = "Previous") }
                        IconButton(onClick = {
                            app.sendMediaCommand(device, uniffi.connected_ffi.MediaCommand.PLAY_PAUSE)
                        }) { Icon(painterResource(R.drawable.ic_play), contentDescription = "Play/Pause") }
                        IconButton(onClick = {
                            app.sendMediaCommand(device, uniffi.connected_ffi.MediaCommand.NEXT)
                        }) { Icon(painterResource(R.drawable.ic_next), contentDescription = "Next") }
                    }

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
    }
}
