package com.connected.app

import android.Manifest
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Environment
import android.provider.OpenableColumns
import android.util.Log
import android.widget.Button
import android.widget.EditText
import android.widget.TextView
import android.widget.Toast
import androidx.activity.result.contract.ActivityResultContracts
import androidx.appcompat.app.AlertDialog
import androidx.appcompat.app.AppCompatActivity
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import androidx.lifecycle.lifecycleScope
import androidx.recyclerview.widget.LinearLayoutManager
import androidx.recyclerview.widget.RecyclerView
import com.connected.core.ClipboardEvent
import com.connected.core.ConnectedSdk
import com.connected.core.DiscoveredDevice
import com.connected.core.DiscoveryEvent
import com.connected.core.FileTransferEvent
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import java.io.File
import java.io.FileOutputStream

class MainActivity : AppCompatActivity() {

    companion object {
        private const val TAG = "MainActivity"
        private const val PERMISSION_REQUEST_CODE = 1001
    }

    private val sdk = ConnectedSdk.getInstance()
    private val deviceAdapter = DeviceAdapter { device -> onDeviceClick(device) }

    private lateinit var tvDeviceInfo: TextView
    private lateinit var tvStatus: TextView
    private lateinit var btnStartDiscovery: Button
    private lateinit var btnStopDiscovery: Button
    private lateinit var btnClipboardSync: Button
    private lateinit var rvDevices: RecyclerView

    private var selectedDevice: DiscoveredDevice? = null
    private var sdkInitialized = false
    private var syncDevice: DiscoveredDevice? = null
    private var cleanupJob: Job? = null

    // File picker launcher
    private val filePickerLauncher = registerForActivityResult(
        ActivityResultContracts.GetContent()
    ) { uri: Uri? ->
        uri?.let { handleFileSelection(it) }
    }

    // Essential permissions required for core functionality (mDNS + network)
    private val essentialPermissions: List<String>
        get() = buildList {
            add(Manifest.permission.INTERNET)
            add(Manifest.permission.ACCESS_NETWORK_STATE)
            add(Manifest.permission.ACCESS_WIFI_STATE)
            add(Manifest.permission.CHANGE_WIFI_MULTICAST_STATE)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                add(Manifest.permission.NEARBY_WIFI_DEVICES)
            }
        }

    // Optional permissions for file access (app works without them, just can't send files)
    private val optionalPermissions: List<String>
        get() = buildList {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                add(Manifest.permission.READ_MEDIA_IMAGES)
                add(Manifest.permission.READ_MEDIA_VIDEO)
                add(Manifest.permission.READ_MEDIA_AUDIO)
            } else if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
                add(Manifest.permission.READ_EXTERNAL_STORAGE)
            }
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_main)

        initViews()
        checkPermissions()
    }

    private fun initViews() {
        tvDeviceInfo = findViewById(R.id.tvDeviceInfo)
        tvStatus = findViewById(R.id.tvStatus)
        btnStartDiscovery = findViewById(R.id.btnStartDiscovery)
        btnStopDiscovery = findViewById(R.id.btnStopDiscovery)
        btnClipboardSync = findViewById(R.id.btnClipboardSync)
        rvDevices = findViewById(R.id.rvDevices)

        rvDevices.layoutManager = LinearLayoutManager(this)
        rvDevices.adapter = deviceAdapter

        // Hide discovery buttons - discovery is now automatic
        btnStartDiscovery.visibility = android.view.View.GONE
        btnStopDiscovery.visibility = android.view.View.GONE

        btnClipboardSync.setOnClickListener { toggleClipboardSync() }

        // Disable clipboard sync button until SDK is initialized
        btnClipboardSync.isEnabled = false
    }

    private fun checkPermissions() {
        val allPermissions = essentialPermissions + optionalPermissions
        val notGranted = allPermissions.filter {
            ContextCompat.checkSelfPermission(this, it) != PackageManager.PERMISSION_GRANTED
        }

        if (notGranted.isNotEmpty()) {
            Log.d(TAG, "Requesting permissions: $notGranted")
            ActivityCompat.requestPermissions(
                this,
                notGranted.toTypedArray(),
                PERMISSION_REQUEST_CODE
            )
        } else {
            Log.d(TAG, "All permissions already granted")
            initializeSdk()
        }
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode == PERMISSION_REQUEST_CODE) {
            // Create a map of permission to grant result
            val results = permissions.zip(grantResults.toList()).toMap()

            // Check if essential permissions are granted
            val essentialGranted = essentialPermissions.all { perm ->
                results[perm] == PackageManager.PERMISSION_GRANTED ||
                        ContextCompat.checkSelfPermission(this, perm) == PackageManager.PERMISSION_GRANTED
            }

            // Log which permissions were denied
            val denied = results.filter { it.value != PackageManager.PERMISSION_GRANTED }
            if (denied.isNotEmpty()) {
                Log.w(TAG, "Permissions denied: ${denied.keys}")
            }

            if (essentialGranted) {
                Log.i(TAG, "Essential permissions granted, initializing SDK")
                initializeSdk()

                // Warn if optional permissions are missing
                val optionalDenied = optionalPermissions.filter {
                    ContextCompat.checkSelfPermission(this, it) != PackageManager.PERMISSION_GRANTED
                }
                if (optionalDenied.isNotEmpty()) {
                    Log.w(TAG, "Optional permissions denied (file sending may be limited): $optionalDenied")
                }
            } else {
                val missingEssential = essentialPermissions.filter {
                    ContextCompat.checkSelfPermission(this, it) != PackageManager.PERMISSION_GRANTED
                }
                Log.e(TAG, "Essential permissions denied: $missingEssential")
                Toast.makeText(
                    this,
                    "Network permissions required for device discovery",
                    Toast.LENGTH_LONG
                ).show()
                updateStatus("Permissions denied - tap to retry")

                // Allow user to retry by tapping the status
                tvStatus.setOnClickListener {
                    checkPermissions()
                }
            }
        }
    }

    private fun initializeSdk() {
        try {
            val deviceName = "${Build.MANUFACTURER} ${Build.MODEL}"
            Log.i(TAG, "Initializing SDK with device name: $deviceName")

            sdk.initialize(this, deviceName)
            sdkInitialized = true

            val localDevice = sdk.getLocalDevice()
            tvDeviceInfo.text =
                "Device: ${localDevice.name}\nIP: ${localDevice.ip}\nPort: ${localDevice.port}"
            updateStatus("🔍 Discovering devices...")

            // Remove retry click listener if it was set
            tvStatus.setOnClickListener(null)

            // Enable clipboard sync button now that SDK is ready
            btnClipboardSync.isEnabled = true

            // Enable pairing mode by default for now
            sdk.setPairingMode(true)

            observeDiscoveryEvents()
            observeFileTransferEvents()
            observeClipboardEvents()
            observePairingEvents()

            // Auto-start discovery so devices can find each other immediately
            startDiscovery()

            // Start periodic cleanup to remove stale devices
            startDeviceCleanup()

        } catch (e: Exception) {
            Log.e(TAG, "Failed to initialize SDK", e)
            sdkInitialized = false
            tvDeviceInfo.text = "Device: Failed to initialize"
            updateStatus("Init failed: ${e.message}")
            Toast.makeText(this, getString(R.string.error_init_failed, e.message), Toast.LENGTH_LONG).show()
        }
    }

    private fun observeDiscoveryEvents() {
        lifecycleScope.launch {
            sdk.discoveryEvents.collect { event ->
                when (event) {
                    is DiscoveryEvent.DeviceFound -> {
                        Log.d(TAG, "Device found: ${event.device.name}")
                        deviceAdapter.addDevice(event.device)
                        val count = deviceAdapter.itemCount
                        updateStatus("📱 $count device${if (count != 1) "s" else ""} nearby")
                    }

                    is DiscoveryEvent.DeviceLost -> {
                        Log.d(TAG, "Device lost: ${event.deviceId}")
                        deviceAdapter.removeDevice(event.deviceId)
                        val count = deviceAdapter.itemCount
                        if (count > 0) {
                            updateStatus("📱 $count device${if (count != 1) "s" else ""} nearby")
                        } else {
                            updateStatus("🔍 Discovering devices...")
                        }
                    }

                    is DiscoveryEvent.Error -> {
                        Log.e(TAG, "Discovery error: ${event.message}")
                        // Don't show error in status, just log it
                    }
                }
            }
        }
    }

    private fun observeFileTransferEvents() {
        lifecycleScope.launch {
            sdk.fileTransferEvents.collect { event ->
                when (event) {
                    is FileTransferEvent.Starting -> {
                        updateStatus("📁 Transfer starting: ${event.filename}")
                        Toast.makeText(
                            this@MainActivity,
                            "Receiving: ${event.filename}",
                            Toast.LENGTH_SHORT
                        ).show()
                    }

                    is FileTransferEvent.Progress -> {
                        val percent = (event.bytesTransferred.toDouble() / event.totalSize * 100).toInt()
                        updateStatus("📁 Transfer: $percent%")
                    }

                    is FileTransferEvent.Completed -> {
                        updateStatus("✅ Received: ${event.filename}")
                        Toast.makeText(
                            this@MainActivity,
                            "File received: ${event.filename}",
                            Toast.LENGTH_LONG
                        ).show()
                    }

                    is FileTransferEvent.Failed -> {
                        updateStatus("❌ Transfer failed: ${event.error}")
                        Toast.makeText(
                            this@MainActivity,
                            "Transfer failed: ${event.error}",
                            Toast.LENGTH_LONG
                        ).show()
                    }

                    is FileTransferEvent.Cancelled -> {
                        updateStatus("⚠️ Transfer cancelled")
                    }
                }
            }
        }
    }

    private fun observePairingEvents() {
        lifecycleScope.launch {
            sdk.pairingEvents.collect { event ->
                when (event) {
                    is com.connected.core.PairingEvent.Request -> {
                        runOnUiThread {
                            showPairingDialog(event)
                        }
                    }
                }
            }
        }
    }

    private fun showPairingDialog(request: com.connected.core.PairingEvent.Request) {
        AlertDialog.Builder(this)
            .setTitle("🔐 Pairing Request")
            .setMessage("${request.deviceName} wants to connect.\nFingerprint: ${request.fingerprint.take(8)}...")
            .setPositiveButton("Trust") { _, _ ->
                sdk.trustDevice(request.fingerprint, request.deviceName)
                Toast.makeText(this, "Trusted ${request.deviceName}", Toast.LENGTH_SHORT).show()
            }
            .setNegativeButton("Block") { _, _ ->
                sdk.blockDevice(request.fingerprint)
                Toast.makeText(this, "Blocked ${request.deviceName}", Toast.LENGTH_SHORT).show()
            }
            .setNeutralButton("Ignore", null)
            .show()
    }

    private fun startDiscovery() {
        try {
            // Clear any stale devices from previous sessions
            deviceAdapter.clear()
            sdk.startDiscovery()
            Log.i(TAG, "Discovery started")
            updateStatus("🔍 Discovering devices...")
        } catch (e: Exception) {
            Log.e(TAG, "Failed to start discovery", e)
        }
    }

    private fun stopDiscovery() {
        try {
            sdk.stopDiscovery()
            Log.i(TAG, "Discovery stopped")
        } catch (e: Exception) {
            Log.e(TAG, "Failed to stop discovery", e)
        }
    }

    private fun startDeviceCleanup() {
        cleanupJob?.cancel()
        cleanupJob = lifecycleScope.launch {
            while (true) {
                delay(5_000) // Check every 5 seconds

                if (!sdkInitialized) continue

                try {
                    // Get current discovered devices from the core
                    val coreDevices = sdk.getDiscoveredDevices()

                    // Remove devices from adapter that are no longer in core's list
                    runOnUiThread {
                        deviceAdapter.removeStaleDevices(coreDevices)
                        val count = deviceAdapter.itemCount
                        if (count > 0) {
                            updateStatus("📱 $count device${if (count != 1) "s" else ""} nearby")
                        } else {
                            updateStatus("🔍 Discovering devices...")
                        }
                    }
                } catch (e: Exception) {
                    Log.w(TAG, "Error during device cleanup: ${e.message}")
                }
            }
        }
    }

    private fun stopDeviceCleanup() {
        cleanupJob?.cancel()
        cleanupJob = null
    }

    private fun observeClipboardEvents() {
        lifecycleScope.launch {
            sdk.clipboardEvents.collect { event ->
                when (event) {
                    is ClipboardEvent.Received -> {
                        Log.d(TAG, "Clipboard received from ${event.fromDevice}")
                        runOnUiThread {
                            showClipboardReceivedDialog(event.text, event.fromDevice)
                        }
                    }

                    is ClipboardEvent.Sent -> {
                        if (event.success) {
                            updateStatus("📋 Clipboard sent!")
                            Toast.makeText(this@MainActivity, "Clipboard sent!", Toast.LENGTH_SHORT).show()
                        } else {
                            updateStatus("❌ Clipboard failed: ${event.error}")
                            Toast.makeText(this@MainActivity, "Failed: ${event.error}", Toast.LENGTH_SHORT).show()
                        }
                    }
                }
            }
        }
    }

    private fun showClipboardReceivedDialog(text: String, fromDevice: String) {
        // If sync is running, auto-copy without dialog
        if (sdk.isClipboardSyncRunning()) {
            sdk.updateSyncedClipboard(this, text)
            updateStatus("📋 Synced: ${text.take(30)}...")
            return
        }

        val preview = if (text.length > 200) text.take(200) + "..." else text

        AlertDialog.Builder(this)
            .setTitle("📋 Clipboard from $fromDevice")
            .setMessage(preview)
            .setPositiveButton("Copy") { _, _ ->
                val clipboard = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
                clipboard.setPrimaryClip(ClipData.newPlainText("Connected", text))
                Toast.makeText(this, "Copied to clipboard!", Toast.LENGTH_SHORT).show()
            }
            .setNegativeButton("Dismiss", null)
            .show()
    }

    private fun toggleClipboardSync() {
        if (sdk.isClipboardSyncRunning()) {
            sdk.stopClipboardSync()
            btnClipboardSync.text = "📋 Start Clipboard Sync"
            syncDevice = null
            updateStatus("Clipboard sync stopped")
            Toast.makeText(this, "Clipboard sync stopped", Toast.LENGTH_SHORT).show()
        } else {
            // Show device picker
            val devices = deviceAdapter.getDevices()
            if (devices.isEmpty()) {
                Toast.makeText(this, "No devices discovered. Start discovery first.", Toast.LENGTH_SHORT).show()
                return
            }

            val deviceNames = devices.map { it.name }.toTypedArray()
            AlertDialog.Builder(this)
                .setTitle("Select device to sync clipboard with")
                .setItems(deviceNames) { _, which ->
                    val device = devices[which]
                    syncDevice = device
                    sdk.startClipboardSync(this, device.ip, device.port)
                    btnClipboardSync.text = "📋 Stop Sync (${device.name})"
                    updateStatus("📋 Clipboard sync with ${device.name}")
                    Toast.makeText(this, "Clipboard sync started with ${device.name}", Toast.LENGTH_SHORT).show()
                }
                .setNegativeButton("Cancel", null)
                .show()
        }
    }

    private fun onDeviceClick(device: DiscoveredDevice) {
        // Show action dialog
        AlertDialog.Builder(this)
            .setTitle(device.name)
            .setItems(arrayOf("Ping", "Send File", "Send Clipboard")) { _, which ->
                when (which) {
                    0 -> pingDevice(device)
                    1 -> {
                        selectedDevice = device
                        filePickerLauncher.launch("*/*")
                    }

                    2 -> showSendClipboardDialog(device)
                }
            }
            .show()
    }

    private fun showSendClipboardDialog(device: DiscoveredDevice) {
        val clipboard = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
        val clipText = clipboard.primaryClip?.getItemAt(0)?.text?.toString() ?: ""

        val editText = EditText(this).apply {
            setText(clipText)
            hint = "Enter text to send"
            setPadding(48, 32, 48, 32)
        }

        AlertDialog.Builder(this)
            .setTitle("Send Clipboard to ${device.name}")
            .setView(editText)
            .setPositiveButton("Send") { _, _ ->
                val text = editText.text.toString()
                if (text.isNotEmpty()) {
                    sendClipboard(device, text)
                } else {
                    Toast.makeText(this, "Text is empty", Toast.LENGTH_SHORT).show()
                }
            }
            .setNegativeButton("Cancel", null)
            .show()
    }

    private fun sendClipboard(device: DiscoveredDevice, text: String) {
        updateStatus("📋 Sending clipboard to ${device.name}...")
        try {
            sdk.sendClipboard(device.ip, device.port, text)
        } catch (e: Exception) {
            Log.e(TAG, "Failed to send clipboard", e)
            Toast.makeText(this, "Failed: ${e.message}", Toast.LENGTH_SHORT).show()
        }
    }

    private fun pingDevice(device: DiscoveredDevice) {
        updateStatus("Pinging ${device.name}...")

        lifecycleScope.launch {
            val result = sdk.ping(device.ip, device.port)
            if (result.success) {
                Toast.makeText(
                    this@MainActivity,
                    getString(R.string.ping_success, result.rttMs),
                    Toast.LENGTH_SHORT
                ).show()
                updateStatus("Ping to ${device.name}: ${result.rttMs}ms")
            } else {
                Toast.makeText(
                    this@MainActivity,
                    getString(R.string.ping_failed, result.errorMessage),
                    Toast.LENGTH_SHORT
                ).show()
                updateStatus("Ping failed: ${result.errorMessage}")
            }
        }
    }

    private fun handleFileSelection(uri: Uri) {
        val device = selectedDevice ?: run {
            Toast.makeText(this, "No device selected", Toast.LENGTH_SHORT).show()
            return
        }

        try {
            // Copy file to a temporary location we can access
            val tempFile = copyUriToTempFile(uri)
            if (tempFile != null) {
                updateStatus("Sending to ${device.name}...")
                sdk.sendFile(device.ip, device.port, tempFile.absolutePath)
            } else {
                Toast.makeText(this, "Failed to access file", Toast.LENGTH_SHORT).show()
            }
        } catch (e: Exception) {
            Log.e(TAG, "Failed to send file", e)
            Toast.makeText(this, "Failed to send file: ${e.message}", Toast.LENGTH_SHORT).show()
        }
    }

    private fun copyUriToTempFile(uri: Uri): File? {
        return try {
            val fileName = getFileName(uri) ?: "temp_file"
            val tempFile = File(cacheDir, fileName)

            contentResolver.openInputStream(uri)?.use { input ->
                FileOutputStream(tempFile).use { output ->
                    input.copyTo(output)
                }
            }

            tempFile
        } catch (e: Exception) {
            Log.e(TAG, "Failed to copy file", e)
            null
        }
    }

    private fun getFileName(uri: Uri): String? {
        var name: String? = null

        if (uri.scheme == "content") {
            contentResolver.query(uri, null, null, null, null)?.use { cursor ->
                if (cursor.moveToFirst()) {
                    val index = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                    if (index >= 0) {
                        name = cursor.getString(index)
                    }
                }
            }
        }

        if (name == null) {
            name = uri.path?.substringAfterLast('/')
        }

        return name
    }

    private fun updateStatus(status: String) {
        runOnUiThread {
            tvStatus.text = "Status: $status"
        }
    }

    override fun onDestroy() {
        super.onDestroy()
        stopDeviceCleanup()
        sdk.shutdown()
    }
}
