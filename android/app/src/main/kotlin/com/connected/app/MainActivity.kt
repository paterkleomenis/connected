package com.connected.app

import android.os.Bundle
import android.net.Uri
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import uniffi.connected_ffi.DiscoveredDevice

@OptIn(ExperimentalMaterial3Api::class)
class MainActivity : ComponentActivity() {
    private lateinit var connectedApp: ConnectedApp

    // Add result launcher for file picker
    private val filePickerLauncher = registerForActivityResult(ActivityResultContracts.GetContent()) { uri ->
        uri?.let { selectedUri ->
            // Get the device to send the file to
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
        // Auto-initialize
        connectedApp.initialize()

        setContent {
            ConnectedTheme {
                Surface(modifier = Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
                    if (connectedApp.isBrowsingRemote.value) {
                        RemoteFileBrowser(connectedApp)
                    } else {
                        ConnectedAppScreen(connectedApp, filePickerLauncher, folderPickerLauncher)
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
        Row(verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
            IconButton(onClick = { app.closeRemoteBrowser() }) {
                Text("⬅", style = MaterialTheme.typography.titleLarge)
            }
            Text("Remote Files: ${app.currentRemotePath.value}", style = MaterialTheme.typography.titleMedium)
        }

        LazyColumn(modifier = Modifier.padding(top = 8.dp)) {

            if (app.currentRemotePath.value != "/") {

                item {

                    Card(modifier = Modifier.padding(vertical = 4.dp).fillMaxWidth(), onClick = {

                        val current = app.currentRemotePath.value

                        val parent = current.substringBeforeLast('/').ifEmpty { "/" }

                        // Navigate up

                        app.browseRemoteFiles(app.getBrowsingDevice()!!, parent)

                    }) {

                        Row(modifier = Modifier.padding(16.dp)) {

                            Text("📁 ..")

                        }

                    }

                }

            }

            items(app.remoteFiles) { file ->

                Card(modifier = Modifier.padding(vertical = 4.dp).fillMaxWidth(), onClick = {

                    if (file.entryType == uniffi.connected_ffi.FfiFsEntryType.DIRECTORY) {

                        app.browseRemoteFiles(app.getBrowsingDevice()!!, file.path)

                    }

                }) {

                    Row(
                        modifier = Modifier.padding(16.dp),
                        horizontalArrangement = androidx.compose.foundation.layout.Arrangement.SpaceBetween
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


@OptIn(ExperimentalMaterial3Api::class)

@Composable

fun ConnectedAppScreen(
    connectedApp: ConnectedApp,
    filePickerLauncher: ActivityResultLauncher<String>? = null,
    folderPickerLauncher: ActivityResultLauncher<Uri?>? = null
) {


    val snackbarHostState = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()

    // Show snackbar when another device unpairs us
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
        snackbarHost = { SnackbarHost(snackbarHostState) }
    ) { paddingValues ->
        Column(modifier = Modifier.padding(paddingValues).padding(16.dp)) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = androidx.compose.foundation.layout.Arrangement.SpaceBetween,
                verticalAlignment = androidx.compose.ui.Alignment.CenterVertically
            ) {
                Text("Nearby Devices", style = MaterialTheme.typography.headlineMedium)

                Row(verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                    Text(
                        "Sync Clipboard",
                        style = MaterialTheme.typography.bodySmall,
                        modifier = Modifier.padding(end = 8.dp)
                    )
                    Switch(
                        checked = connectedApp.isClipboardSyncEnabled.value,
                        onCheckedChange = { connectedApp.toggleClipboardSync() }
                    )
                }
            }

            Card(
                colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.secondaryContainer),
                modifier = Modifier.padding(vertical = 8.dp).fillMaxWidth()
            ) {
                Column(modifier = Modifier.padding(16.dp)) {
                    Text("Shared Folder", style = MaterialTheme.typography.titleSmall)

                    if (connectedApp.isFsProviderRegistered.value) {
                        Text(
                            "Sharing: ${connectedApp.sharedFolderName.value ?: "Unknown"}",
                            style = MaterialTheme.typography.bodySmall,
                            modifier = Modifier.padding(bottom = 16.dp)
                        )
                    } else {
                        Text(
                            "Select a sharing mode below.",
                            style = MaterialTheme.typography.bodySmall,
                            modifier = Modifier.padding(bottom = 16.dp)
                        )
                    }

                    if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.R) {
                        Button(
                            colors = ButtonDefaults.buttonColors(containerColor = MaterialTheme.colorScheme.tertiary),
                            onClick = {
                                if (connectedApp.isFullAccessGranted()) {
                                    connectedApp.setFullAccess()
                                } else {
                                    connectedApp.requestFullAccessPermission()
                                }
                            },
                            modifier = Modifier.fillMaxWidth().padding(bottom = 8.dp)
                        ) {
                            Text(if (connectedApp.isFullAccessGranted()) "Use Full Access" else "Grant Full Access")
                        }
                    }

                    Button(
                        onClick = { folderPickerLauncher?.launch(null) },
                        modifier = Modifier.fillMaxWidth()
                    ) {
                        Text("Select Specific Folder")
                    }
                }
            }

            if (connectedApp.devices.isEmpty()) {
                Text(
                    "Searching...",
                    style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.padding(top = 16.dp)
                )
            } else {
                LazyColumn(modifier = Modifier.padding(top = 8.dp)) {
                    items(connectedApp.devices) { device ->
                        DeviceItem(device, connectedApp, filePickerLauncher)
                    }
                }
            }

            Text(
                "Status: ${connectedApp.transferStatus.value}",
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.padding(top = 16.dp)
            )

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
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DeviceItem(
    device: DiscoveredDevice,
    app: ConnectedApp,
    filePickerLauncher: ActivityResultLauncher<String>? = null
) {

    // Check if ID is in the trusted set (observes state change)
    val isTrusted = app.trustedDevices.contains(device.id)
    val isPending = app.pendingPairing.contains(device.id)
    var showMenu by remember { mutableStateOf(false) }

    Card(modifier = Modifier.padding(vertical = 4.dp).fillMaxSize()) {
        Row(
            modifier = Modifier.padding(8.dp).fillMaxSize(),
            horizontalArrangement = androidx.compose.foundation.layout.Arrangement.SpaceBetween,
            verticalAlignment = androidx.compose.ui.Alignment.CenterVertically
        ) {
            Column(modifier = Modifier.weight(1f)) {
                Text(text = device.name, style = MaterialTheme.typography.bodyLarge)
                Text(text = "${device.ip}:${device.port}", style = MaterialTheme.typography.bodySmall)
                if (isTrusted) {
                    Text(
                        text = "Trusted",
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.primary
                    )
                }
            }

            if (isTrusted) {
                Row {
                    // Send file button before menu
                    IconButton(onClick = {
                        // Store the device for later file transfer
                        app.setSelectedDeviceForFileTransfer(device)

                        // Launch file picker if available
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
