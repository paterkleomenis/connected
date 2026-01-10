package com.connected.app

import android.Manifest
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Home
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import kotlinx.coroutines.launch
import uniffi.connected_ffi.DiscoveredDevice

@OptIn(ExperimentalMaterial3Api::class)
class MainActivity : ComponentActivity() {
    private lateinit var connectedApp: ConnectedApp

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

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        connectedApp = ConnectedApp(this)
        connectedApp.initialize()

        setContent {
            ConnectedTheme {
                Surface(modifier = Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
                    if (connectedApp.isBrowsingRemote.value) {
                        RemoteFileBrowser(connectedApp)
                    } else {
                        MainAppNavigation(connectedApp, filePickerLauncher, folderPickerLauncher)
                    }
                }
            }
        }
    }

    override fun onDestroy() {
        connectedApp.cleanup()
        super.onDestroy()
    }
}

@Composable
fun RemoteFileBrowser(app: ConnectedApp) {
    Column(modifier = Modifier.fillMaxSize().padding(16.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = { app.closeRemoteBrowser() }) {
                Text("⬅", style = MaterialTheme.typography.titleLarge)
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
                        Row(modifier = Modifier.padding(16.dp)) {
                            Text("📁 ..")
                        }
                    }
                }
            }

            items(app.remoteFiles) { file ->
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
                        horizontalArrangement = Arrangement.SpaceBetween
                    ) {
                        Row {
                            Text(if (file.entryType == uniffi.connected_ffi.FfiFsEntryType.DIRECTORY) "📁 " else "📄 ")
                            Text(file.name)
                        }
                        Text(if (file.entryType == uniffi.connected_ffi.FfiFsEntryType.DIRECTORY) "" else "${file.size} B")
                    }
                }
            }
        }
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
    folderPickerLauncher: ActivityResultLauncher<Uri?>? = null
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
                    icon = { Icon(Icons.Default.Home, contentDescription = "Devices") },
                    label = { Text("Devices") },
                    selected = currentScreen == Screen.Home,
                    onClick = { currentScreen = Screen.Home }
                )
                NavigationBarItem(
                    icon = { Icon(Icons.Default.Settings, contentDescription = "Settings") },
                    label = { Text("Settings") },
                    selected = currentScreen == Screen.Settings,
                    onClick = { currentScreen = Screen.Settings }
                )
            }
        }
    ) { paddingValues ->
        Box(modifier = Modifier.padding(paddingValues)) {
            when (currentScreen) {
                Screen.Home -> HomeScreen(connectedApp, filePickerLauncher)
                Screen.Settings -> SettingsScreen(connectedApp, folderPickerLauncher)
            }
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

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun HomeScreen(
    connectedApp: ConnectedApp,
    filePickerLauncher: ActivityResultLauncher<String>? = null
) {
    Column(modifier = Modifier.fillMaxSize().padding(16.dp)) {
        Text(
            "Nearby Devices",
            style = MaterialTheme.typography.headlineMedium,
            modifier = Modifier.padding(bottom = 16.dp)
        )

        if (connectedApp.devices.isEmpty()) {
            Box(
                modifier = Modifier.fillMaxSize(),
                contentAlignment = Alignment.Center
            ) {
                Column(horizontalAlignment = Alignment.CenterHorizontally) {
                    Text("📡", style = MaterialTheme.typography.displayLarge)
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
                    DeviceItem(device, connectedApp, filePickerLauncher)
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

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsScreen(
    connectedApp: ConnectedApp,
    folderPickerLauncher: ActivityResultLauncher<Uri?>? = null
) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    var isNotificationAccessGranted by remember { mutableStateOf(false) }

    // Telephony permissions
    var hasTelephonyPermissions by remember { mutableStateOf(false) }
    var permissionsRequested by remember { mutableStateOf(false) }

    DisposableEffect(lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) {
                // Check notification access
                val componentName = android.content.ComponentName(context, MediaObserverService::class.java)
                val enabledListeners = android.provider.Settings.Secure.getString(
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
            androidx.core.content.ContextCompat.checkSelfPermission(context, permission) !=
                    android.content.pm.PackageManager.PERMISSION_GRANTED
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

        // Clipboard Sync Section
        item {
            Card(
                colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant),
                modifier = Modifier.fillMaxWidth()
            ) {
                Column(modifier = Modifier.padding(16.dp)) {
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.SpaceBetween,
                        verticalAlignment = Alignment.CenterVertically
                    ) {
                        Column(modifier = Modifier.weight(1f)) {
                            Text("📋 Clipboard Sync", style = MaterialTheme.typography.titleMedium)
                            Text(
                                "Automatically sync clipboard with trusted devices",
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant
                            )
                        }
                        Switch(
                            checked = connectedApp.isClipboardSyncEnabled.value,
                            onCheckedChange = { connectedApp.toggleClipboardSync() }
                        )
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
                        horizontalArrangement = Arrangement.SpaceBetween,
                        verticalAlignment = Alignment.CenterVertically
                    ) {
                        Column(modifier = Modifier.weight(1f)) {
                            Text("🎵 Media Control", style = MaterialTheme.typography.titleMedium)
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
                                    android.content.Intent("android.settings.ACTION_NOTIFICATION_LISTENER_SETTINGS")
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
                        horizontalArrangement = Arrangement.SpaceBetween,
                        verticalAlignment = Alignment.CenterVertically
                    ) {
                        Column(modifier = Modifier.weight(1f)) {
                            Text("📱 Phone Link", style = MaterialTheme.typography.titleMedium)
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
                    Text("📁 Shared Folder", style = MaterialTheme.typography.titleMedium)
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

                    if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.R) {
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
                            Text(if (connectedApp.isFullAccessGranted()) "📱 Use Full Access" else "🔓 Grant Full Access")
                        }
                        Spacer(modifier = Modifier.height(8.dp))
                    }

                    OutlinedButton(
                        onClick = { folderPickerLauncher?.launch(null) },
                        modifier = Modifier.fillMaxWidth()
                    ) {
                        Text("📂 Select Specific Folder")
                    }

                    if (connectedApp.isFsProviderRegistered.value) {
                        Spacer(modifier = Modifier.height(8.dp))
                        TextButton(
                            onClick = { connectedApp.stopSharing() },
                            modifier = Modifier.fillMaxWidth()
                        ) {
                            Text("🚫 Stop Sharing", color = MaterialTheme.colorScheme.error)
                        }
                    }
                }
            }
        }

        // About Section
        item {
            Card(
                colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant),
                modifier = Modifier.fillMaxWidth()
            ) {
                Column(modifier = Modifier.padding(16.dp)) {
                    Text("ℹ️ About", style = MaterialTheme.typography.titleMedium)
                    Spacer(modifier = Modifier.height(8.dp))
                    Text(
                        "Connected allows you to seamlessly share files, clipboard, control media, send texts, and make calls between your devices.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant
                    )
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
    filePickerLauncher: ActivityResultLauncher<String>? = null
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
            Column(modifier = Modifier.weight(1f)) {
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

            if (isTrusted) {
                Row {
                    // Media Controls (if enabled)
                    if (app.isMediaControlEnabled.value) {
                        IconButton(onClick = {
                            app.sendMediaCommand(device, uniffi.connected_ffi.MediaCommand.PREVIOUS)
                        }) { Text("⏮") }
                        IconButton(onClick = {
                            app.sendMediaCommand(device, uniffi.connected_ffi.MediaCommand.PLAY_PAUSE)
                        }) { Text("⏯") }
                        IconButton(onClick = {
                            app.sendMediaCommand(device, uniffi.connected_ffi.MediaCommand.NEXT)
                        }) { Text("⏭") }
                    }

                    // Send file button
                    IconButton(onClick = {
                        app.setSelectedDeviceForFileTransfer(device)
                        filePickerLauncher?.launch("*/*")
                    }) {
                        Text("📁", style = MaterialTheme.typography.titleLarge)
                    }

                    // More options dropdown
                    Box {
                        IconButton(onClick = { showMenu = true }) {
                            Text("⋮", style = MaterialTheme.typography.titleLarge)
                        }
                        DropdownMenu(
                            expanded = showMenu,
                            onDismissRequest = { showMenu = false }
                        ) {
                            DropdownMenuItem(
                                text = { Text("📋 Send Clipboard") },
                                onClick = {
                                    showMenu = false
                                    app.sendClipboard(device)
                                }
                            )
                            DropdownMenuItem(
                                text = { Text("📂 Browse Files") },
                                onClick = {
                                    showMenu = false
                                    app.browseRemoteFiles(device)
                                }
                            )
                            DropdownMenuItem(
                                text = { Text("💔 Unpair") },
                                onClick = {
                                    showMenu = false
                                    app.unpairDevice(device)
                                }
                            )
                            DropdownMenuItem(
                                text = { Text("🔄 Forget") },
                                onClick = {
                                    showMenu = false
                                    app.forgetDevice(device)
                                }
                            )
                            DropdownMenuItem(
                                text = { Text("🚫 Block", color = MaterialTheme.colorScheme.error) },
                                onClick = {
                                    showMenu = false
                                    app.blockDevice(device)
                                }
                            )
                        }
                    }
                }
            } else if (isPending) {
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
