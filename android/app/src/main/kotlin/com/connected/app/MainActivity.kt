package com.connected.app

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.layout.Box
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import uniffi.connected_ffi.DiscoveredDevice

class MainActivity : ComponentActivity() {
    private lateinit var connectedApp: ConnectedApp

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        connectedApp = ConnectedApp(this)
        // Auto-initialize
        connectedApp.initialize()

        setContent {
            ConnectedTheme {
                Surface(modifier = Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
                    ConnectedAppScreen(connectedApp)
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
fun ConnectedAppScreen(connectedApp: ConnectedApp) {
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
            Text("Nearby Devices", style = MaterialTheme.typography.headlineMedium)

            if (connectedApp.devices.isEmpty()) {
                Text(
                    "Searching...",
                    style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.padding(top = 16.dp)
                )
            } else {
                LazyColumn(modifier = Modifier.padding(top = 8.dp)) {
                    items(connectedApp.devices) { device ->
                        DeviceItem(device, connectedApp)
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

@Composable
fun DeviceItem(device: DiscoveredDevice, app: ConnectedApp) {
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
