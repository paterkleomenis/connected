package com.connected.app

import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothManager
import android.bluetooth.le.AdvertiseCallback
import android.bluetooth.le.AdvertiseData
import android.bluetooth.le.AdvertiseSettings
import android.bluetooth.le.BluetoothLeAdvertiser
import android.bluetooth.le.BluetoothLeScanner
import android.bluetooth.le.ScanCallback
import android.bluetooth.le.ScanFilter
import android.bluetooth.le.ScanResult
import android.bluetooth.le.ScanSettings
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.location.LocationManager
import android.net.wifi.p2p.WifiP2pConfig
import android.net.wifi.p2p.WifiP2pDevice
import android.net.wifi.p2p.WifiP2pManager
import android.net.wifi.WpsInfo
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.ParcelUuid
import android.os.SystemClock
import android.provider.Settings
import android.util.Log
import androidx.core.content.ContextCompat
import uniffi.connected_ffi.DiscoveredDevice
import uniffi.connected_ffi.getLocalDevice
import uniffi.connected_ffi.injectProximityDevice
import java.net.InetAddress
import java.nio.ByteBuffer
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.text.Normalizer

class ProximityManager(private val context: Context) {
    companion object {
        private const val TAG = "ProximityManager"
        private const val MANUFACTURER_ID = 0xFFFF
        private const val PROTOCOL_VERSION = 3
        private const val MIN_COMPATIBLE_VERSION = 1
        private const val NAME_MAX_LEN = 20
        private const val UNKNOWN_IP = "0.0.0.0"
        private const val SYNTHETIC_IP_PREFIX = "198.18."
        private const val CONNECT_COOLDOWN_MS = 15_000L
        private const val DISCOVERY_COOLDOWN_MS = 15_000L
        private const val RETRY_DELAY_MS = 5_000L
        private const val CONNECT_RETRY_DELAY_MS = 2_000L
        private const val GROUP_REFRESH_DELAY_MS = 2_000L
        private const val GROUP_CREATE_COOLDOWN_MS = 15_000L
        private const val GROUP_RECOVERY_DELAY_MS = 2_000L
        private const val PAIR_INTENT_TTL_MS = 30_000L
        private const val PAIR_INTENT_DEBOUNCE_MS = 10_000L
        private val NAME_SERVICE_UUID =
            ParcelUuid(UUID.fromString("0000FD00-0000-1000-8000-00805F9B34FB"))
    }

    private val bluetoothManager =
        context.getSystemService(Context.BLUETOOTH_SERVICE) as BluetoothManager
    private val bluetoothAdapter: BluetoothAdapter? = bluetoothManager.adapter

    private var advertiser: BluetoothLeAdvertiser? = null
    private var scanner: BluetoothLeScanner? = null
    private var advertiseCallback: AdvertiseCallback? = null
    private var scanCallback: ScanCallback? = null

    private val wifiP2pManager =
        context.getSystemService(Context.WIFI_P2P_SERVICE) as WifiP2pManager?
    private var p2pChannel: WifiP2pManager.Channel? = null
    private var p2pReceiver: BroadcastReceiver? = null
    private val handler = Handler(Looper.getMainLooper())
    private val retryDiscoveryRunnable = Runnable { discoverPeers(force = true) }
    private val retryConnectRunnable = Runnable { discoverPeers(force = true) }
    private val retryGroupCreateRunnable = Runnable { createGroupIfNeeded(force = true) }

    private val peersById = ConcurrentHashMap<String, ProximityPeer>()
    private var lastAdvertisedSignature: String? = null

    @Volatile
    private var pendingPeerId: String? = null

    @Volatile
    private var pendingPeerName: String? = null

    @Volatile
    private var pendingPreferGroupOwner = false
    private var lastConnectAttempt = 0L
    private var lastDiscoveryAttempt = 0L
    private var lastGroupCreateAttempt = 0L
    private var localP2pDeviceName: String? = null
    private var groupCreateRetryAttempts = 0

    @Volatile
    private var pairingActiveUntil = 0L

    @Volatile
    private var pairingTargetId: String? = null

    @Volatile
    private var p2pActionInFlight = false
    private val lastPairIntentHandledAt = ConcurrentHashMap<String, Long>()
    private val lastBleLogAt = ConcurrentHashMap<String, Long>()

    @Volatile
    private var p2pConnected = false

    @Volatile
    private var p2pIsGroupOwner = false

    @Volatile
    private var p2pEnabled = true

    var onPairingIntent: ((String) -> Unit)? = null

    data class ProximityPeer(
        val deviceId: String,
        val name: String,
        val deviceType: String,
        val port: Int,
        val protocolVersion: Int,
        val ip: String? = null,
        val matchName: String = name,
        val pairingIntent: Boolean = false,
    )

    fun start() {
        startBle()
        startWifiDirect()
    }

    fun stop() {
        stopWifiDirect()
        stopBle()
    }

    fun requestConnect(deviceId: String) {
        val peer = peersById[deviceId]
        if (peer == null) {
            Log.d(TAG, "No proximity peer for device id $deviceId")
            return
        }
        Log.d(TAG, "Proximity connect requested for ${peer.matchName} ($deviceId)")
        pairingActiveUntil = SystemClock.elapsedRealtime() + PAIR_INTENT_TTL_MS
        pairingTargetId = deviceId
        pendingPeerId = deviceId
        pendingPeerName = peer.matchName
        pendingPreferGroupOwner = shouldBeGroupOwner(deviceId)
        refreshAdvertising(force = true)
        handler.postDelayed({ refreshAdvertising(force = true) }, PAIR_INTENT_TTL_MS + 100)
        maybeConnectWifiDirect(peer, force = true)
    }

    private fun startBle() {
        val adapter = bluetoothAdapter
        val enabled = try {
            adapter?.isEnabled == true
        } catch (e: SecurityException) {
            Log.w(TAG, "Missing Bluetooth connect permission", e)
            false
        }
        if (adapter == null || !enabled) {
            Log.w(TAG, "Bluetooth unavailable or disabled")
            return
        }

        advertiser = adapter.bluetoothLeAdvertiser
        scanner = adapter.bluetoothLeScanner
        if (advertiser == null || scanner == null) {
            Log.w(TAG, "BLE advertiser or scanner not available")
            return
        }

        refreshAdvertising()
        startScanning()
    }

    private fun stopBle() {
        stopAdvertising()
        stopScanning()
    }

    private fun refreshAdvertising(force: Boolean = false) {
        if (!hasBleAdvertisePermission()) {
            Log.w(TAG, "Missing BLE advertise permission")
            return
        }

        val local = runCatching { getLocalDevice() }.getOrNull() ?: return
        val pairingFlag = if (SystemClock.elapsedRealtime() < pairingActiveUntil) 1 else 0
        val signature =
            "${local.id}|${local.name}|${local.deviceType}|${local.port}|$pairingFlag"

        if (!force && signature == lastAdvertisedSignature && advertiseCallback != null) {
            return
        }

        stopAdvertising()

        val payload = buildPayload(local)
        val nameData = buildNameData(local)
        val settings = AdvertiseSettings.Builder()
            .setAdvertiseMode(AdvertiseSettings.ADVERTISE_MODE_LOW_LATENCY)
            .setTxPowerLevel(AdvertiseSettings.ADVERTISE_TX_POWER_MEDIUM)
            .setConnectable(false)
            .build()

        val data = AdvertiseData.Builder()
            .addManufacturerData(MANUFACTURER_ID, payload)
            .build()

        val scanResponse = AdvertiseData.Builder()
            .addServiceData(NAME_SERVICE_UUID, nameData)
            .build()

        advertiseCallback = object : AdvertiseCallback() {
            override fun onStartFailure(errorCode: Int) {
                Log.w(TAG, "BLE advertise failed: $errorCode")
            }

            override fun onStartSuccess(settingsInEffect: AdvertiseSettings) {
                Log.d(TAG, "BLE advertise started")
            }
        }

        advertiser?.startAdvertising(settings, data, scanResponse, advertiseCallback)
        lastAdvertisedSignature = signature
    }

    private fun stopAdvertising() {
        val callback = advertiseCallback
        if (callback != null) {
            try {
                advertiser?.stopAdvertising(callback)
            } catch (e: Exception) {
                Log.w(TAG, "Failed to stop advertising: ${e.message}")
            }
        }
        advertiseCallback = null
        lastAdvertisedSignature = null
    }

    private fun startScanning() {
        if (!hasBleScanPermission()) {
            Log.w(TAG, "Missing BLE scan permission")
            return
        }

        val filters = listOf(
            ScanFilter.Builder()
                .setManufacturerData(MANUFACTURER_ID, ByteArray(0))
                .build()
        )
        val settings = ScanSettings.Builder()
            .setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY)
            .build()

        scanCallback = object : ScanCallback() {
            override fun onScanResult(callbackType: Int, result: ScanResult) {
                handleScanResult(result)
            }

            override fun onBatchScanResults(results: MutableList<ScanResult>) {
                results.forEach { handleScanResult(it) }
            }

            override fun onScanFailed(errorCode: Int) {
                Log.w(TAG, "BLE scan failed: $errorCode")
            }
        }

        scanner?.startScan(filters, settings, scanCallback)
    }

    private fun stopScanning() {
        val callback = scanCallback
        if (callback != null) {
            try {
                scanner?.stopScan(callback)
            } catch (e: Exception) {
                Log.w(TAG, "Failed to stop scanning: ${e.message}")
            }
        }
        scanCallback = null
    }

    private fun handleScanResult(result: ScanResult) {
        val record = result.scanRecord ?: return
        val payload = record.getManufacturerSpecificData(MANUFACTURER_ID) ?: return
        val serviceNameData = record.getServiceData(NAME_SERVICE_UUID) ?: return
        val nameOverride = String(serviceNameData, Charsets.UTF_8)
            .ifBlank { record.deviceName ?: result.device.name }
        val peer = parsePayload(payload, nameOverride) ?: return

        val localId = runCatching { getLocalDevice().id }.getOrNull()
        if (peer.deviceId == localId) {
            return
        }

        val now = SystemClock.elapsedRealtime()
        val lastLog = lastBleLogAt[peer.deviceId] ?: 0L
        if (now - lastLog > 2000L) {
            Log.d(TAG, "BLE peer: ${peer.matchName} (id=${peer.deviceId})")
            lastBleLogAt[peer.deviceId] = now
        }
        peersById[peer.deviceId] = peer

        if (peer.pairingIntent) {
            val now = SystemClock.elapsedRealtime()
            val lastHandled = lastPairIntentHandledAt[peer.deviceId] ?: 0L
            if (now - lastHandled < PAIR_INTENT_DEBOUNCE_MS) {
                return
            }
            lastPairIntentHandledAt[peer.deviceId] = now
            if (p2pActionInFlight) {
                Log.d(TAG, "Pair intent ignored; Wi-Fi Direct action already in flight")
                return
            }
            // Notify listener about the pairing intent so the app can prepare to handshake
            handler.post { onPairingIntent?.invoke(peer.deviceId) }

            pendingPeerId = peer.deviceId
            pendingPeerName = peer.matchName
            pendingPreferGroupOwner = shouldBeGroupOwner(peer.deviceId)
            if (pendingPreferGroupOwner) {
                Log.d(TAG, "Pair intent: auto group-owner for ${peer.matchName}")
                createGroupIfNeeded(force = true)
            } else {
                Log.d(TAG, "Pair intent: auto connect for ${peer.matchName}")
                maybeConnectWifiDirect(peer, force = true)
            }
        }

        val ipForInject = if (isUsableIp(peer.ip)) {
            peer.ip!!
        } else {
            syntheticIpForDevice(peer.deviceId)
        }
        try {
            injectProximityDevice(
                peer.deviceId,
                peer.name,
                peer.deviceType,
                ipForInject,
                peer.port.toUShort()
            )
        } catch (e: Exception) {
            Log.w(TAG, "Failed to inject proximity device", e)
        }
    }

    private fun startWifiDirect() {
        if (wifiP2pManager == null) {
            return
        }

        if (!hasP2pPermission()) {
            Log.w(TAG, "Missing Wi-Fi Direct permissions")
            return
        }

        if (!isLocationEnabled()) {
            Log.w(TAG, "Location services are disabled; Wi-Fi Direct discovery may fail")
        }

        p2pChannel = wifiP2pManager.initialize(context, context.mainLooper, null)

        val filter = IntentFilter().apply {
            addAction(WifiP2pManager.WIFI_P2P_STATE_CHANGED_ACTION)
            addAction(WifiP2pManager.WIFI_P2P_PEERS_CHANGED_ACTION)
            addAction(WifiP2pManager.WIFI_P2P_CONNECTION_CHANGED_ACTION)
            addAction(WifiP2pManager.WIFI_P2P_THIS_DEVICE_CHANGED_ACTION)
        }

        p2pReceiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context?, intent: Intent?) {
                val action = intent?.action ?: return
                when (action) {
                    WifiP2pManager.WIFI_P2P_STATE_CHANGED_ACTION -> {
                        val state = intent.getIntExtra(
                            WifiP2pManager.EXTRA_WIFI_STATE,
                            WifiP2pManager.WIFI_P2P_STATE_DISABLED
                        )
                        p2pEnabled = state == WifiP2pManager.WIFI_P2P_STATE_ENABLED
                        if (state == WifiP2pManager.WIFI_P2P_STATE_ENABLED) {
                            discoverPeers()
                        }
                    }

                    WifiP2pManager.WIFI_P2P_PEERS_CHANGED_ACTION -> {
                        requestPeers()
                    }

                    WifiP2pManager.WIFI_P2P_CONNECTION_CHANGED_ACTION -> {
                        handleConnectionChanged()
                    }

                    WifiP2pManager.WIFI_P2P_THIS_DEVICE_CHANGED_ACTION -> {
                        val name = readThisDeviceName(intent)
                        if (!name.isNullOrBlank()) {
                            localP2pDeviceName = name
                            Log.d(TAG, "Wi-Fi Direct local name: $name")
                        }
                    }
                }
            }
        }

        context.registerReceiver(p2pReceiver, filter)
        try {
            discoverPeers()
        } catch (e: Exception) {
            Log.w(TAG, "Failed to start discovery in startWifiDirect: ${e.message}")
            scheduleDiscoveryRetry()
        }
    }

    private fun stopWifiDirect() {
        p2pReceiver?.let { receiver ->
            runCatching { context.unregisterReceiver(receiver) }
        }
        p2pReceiver = null
        p2pChannel = null
        p2pConnected = false
        p2pIsGroupOwner = false
        pendingPreferGroupOwner = false
        handler.removeCallbacks(retryDiscoveryRunnable)
    }

    private fun discoverPeers(force: Boolean = false) {
        val manager = wifiP2pManager ?: return
        val channel = p2pChannel ?: return
        if (!hasP2pPermission()) {
            Log.w(TAG, "Missing Wi-Fi Direct permissions")
            return
        }

        val now = SystemClock.elapsedRealtime()
        if (!force && now - lastDiscoveryAttempt < DISCOVERY_COOLDOWN_MS) {
            return
        }
        lastDiscoveryAttempt = now

        try {
            manager.discoverPeers(channel, object : WifiP2pManager.ActionListener {
                override fun onSuccess() {
                    Log.d(TAG, "Wi-Fi Direct peer discovery started")
                    handler.postDelayed({ requestPeers() }, 1000L)
                }

                override fun onFailure(reason: Int) {
                    handleP2pFailure("peer discovery", reason)
                }
            })
        } catch (e: Exception) {
            Log.w(TAG, "discoverPeers failed: ${e.message}")
        }
    }

    private fun requestPeers() {
        val manager = wifiP2pManager ?: return
        val channel = p2pChannel ?: return
        try {
            manager.requestPeers(channel) { peers ->
                val candidates = peers.deviceList.toList()
                handler.post { handlePeerList(candidates) }
            }
        } catch (e: Exception) {
            Log.w(TAG, "requestPeers failed: ${e.message}")
        }
    }

    private fun handlePeerList(candidates: List<WifiP2pDevice>) {
        if (p2pConnected) {
            return
        }
        if (pendingPeerId == null && pairingTargetId != null) {
            pendingPeerId = pairingTargetId
            pendingPeerName = pendingPeerId?.let { peersById[it]?.matchName }
            pendingPreferGroupOwner = pendingPeerId?.let { shouldBeGroupOwner(it) } == true
            Log.d(TAG, "Recovered pending target from pairing intent")
        }
        val hasPendingTarget = pendingPeerId != null || pairingTargetId != null
        if (!hasPendingTarget) {
            Log.d(TAG, "Wi-Fi Direct peers available but no pending target; skipping connect")
            return
        }
        if (candidates.isEmpty()) {
            return
        }

        val targetName = pendingPeerName ?: pairingTargetId?.let { peersById[it]?.matchName }
        val match = candidates.firstOrNull { candidate ->
            namesMatch(targetName, candidate.deviceName)
        }
        val candidate = when {
            match != null -> match
            !hasPendingTarget -> candidates.firstOrNull()
            else -> null
        }

        Log.d(
            TAG,
            "Wi-Fi Direct peers: ${
                candidates.joinToString { "${it.deviceName}(${it.deviceAddress})" }
            }"
        )
        if (pendingPreferGroupOwner) {
            Log.d(TAG, "Acting as group owner; waiting for peer to connect")
            return
        }
        if (candidate == null) {
            Log.d(
                TAG,
                "No Wi-Fi Direct peer matched '$targetName' (candidates=${candidates.size})"
            )
            if (hasPendingTarget) {
                scheduleDiscoveryRetry()
            }
            return
        }
        connectToPeer(candidate)
    }

    private fun connectToPeer(device: WifiP2pDevice) {
        val manager = wifiP2pManager ?: return
        val channel = p2pChannel ?: return
        if (p2pActionInFlight) {
            return
        }
        p2pActionInFlight = true

        val config = WifiP2pConfig().apply {
            deviceAddress = device.deviceAddress
            wps.setup = WpsInfo.PBC
            groupOwnerIntent = 0
        }

        Log.d(TAG, "Wi-Fi Direct connect to ${device.deviceName} (${device.deviceAddress})")

        val doConnect = {
            try {
                manager.connect(channel, config, object : WifiP2pManager.ActionListener {
                    override fun onSuccess() {
                        Log.d(TAG, "Wi-Fi Direct connect requested")
                        p2pActionInFlight = false
                    }

                    override fun onFailure(reason: Int) {
                        p2pActionInFlight = false
                        handleP2pFailure("connect", reason)
                    }
                })
            } catch (e: Exception) {
                Log.w(TAG, "connect failed: ${e.message}")
                p2pActionInFlight = false
            }
        }

        // On many devices, calling stopPeerDiscovery manually causes race conditions/BUSY errors.
        // The framework typically handles stopping discovery when connect is called.
        doConnect()
    }

    private fun handleConnectionChanged() {
        val manager = wifiP2pManager ?: return
        val channel = p2pChannel ?: return
        manager.requestConnectionInfo(channel) { info ->
            p2pConnected = info.groupFormed
            p2pIsGroupOwner = info.groupFormed && info.isGroupOwner
            if (info.groupFormed) {
                refreshAdvertising()
                if (!info.isGroupOwner) {
                    val peerId = pendingPeerId
                    val peer = peerId?.let { peersById[it] }
                    val groupOwnerIp = info.groupOwnerAddress?.hostAddress
                    if (peer != null && !groupOwnerIp.isNullOrEmpty()) {
                        try {
                            injectProximityDevice(
                                peer.deviceId,
                                peer.name,
                                peer.deviceType,
                                groupOwnerIp,
                                peer.port.toUShort()
                            )
                        } catch (e: Exception) {
                            Log.w(TAG, "Failed to update group owner endpoint", e)
                        }
                    }
                }
            } else {
                refreshAdvertising()
            }
        }
    }

    private fun maybeConnectWifiDirect(peer: ProximityPeer, force: Boolean) {
        if (wifiP2pManager == null || !hasP2pPermission() || !p2pEnabled) {
            Log.d(
                TAG,
                "Wi-Fi Direct connect skipped (p2pManager=${wifiP2pManager != null}, connected=$p2pConnected, permission=${hasP2pPermission()}, enabled=$p2pEnabled)"
            )
            return
        }

        if (!isLocationEnabled()) {
            Log.w(TAG, "Location services are disabled; Wi-Fi Direct discovery may fail")
        }

        val now = SystemClock.elapsedRealtime()
        if (!force && now - lastConnectAttempt < CONNECT_COOLDOWN_MS) {
            return
        }

        pendingPeerId = peer.deviceId
        pendingPeerName = peer.matchName
        pendingPreferGroupOwner = shouldBeGroupOwner(peer.deviceId)
        lastConnectAttempt = now
        if (p2pConnected) {
            Log.d(TAG, "Wi-Fi Direct already connected; skipping connect")
            return
        }
        if (pendingPreferGroupOwner) {
            createGroupIfNeeded(force)
        } else {
            discoverPeers(force = force)
        }
    }

    private fun handleP2pFailure(action: String, reason: Int) {
        val reasonLabel = when (reason) {
            WifiP2pManager.BUSY -> "BUSY"
            WifiP2pManager.P2P_UNSUPPORTED -> "UNSUPPORTED"
            WifiP2pManager.ERROR -> "ERROR"
            else -> reason.toString()
        }
        Log.w(TAG, "Wi-Fi Direct $action failed: $reasonLabel")
        if ((reason == WifiP2pManager.BUSY || reason == WifiP2pManager.ERROR) || (action == "connect" && pendingPeerId != null)) {
            scheduleDiscoveryRetry(force = true)
        }
        if (action == "connect" && pendingPeerId != null) {
            if (reason == WifiP2pManager.ERROR) {
                recoverFromConnectError()
            }
            scheduleConnectRetry()
        }
        if (action == "create group" &&
            (reason == WifiP2pManager.BUSY || reason == WifiP2pManager.ERROR)
        ) {
            recoverFromGroupError()
        }
    }

    private fun scheduleDiscoveryRetry(force: Boolean = false) {
        handler.removeCallbacks(retryDiscoveryRunnable)
        handler.postDelayed(retryDiscoveryRunnable, if (force) CONNECT_RETRY_DELAY_MS else RETRY_DELAY_MS)
    }

    private fun scheduleConnectRetry() {
        handler.removeCallbacks(retryConnectRunnable)
        handler.postDelayed(retryConnectRunnable, CONNECT_RETRY_DELAY_MS)
    }

    private fun recoverFromConnectError() {
        val manager = wifiP2pManager ?: return
        val channel = p2pChannel ?: return
        if (!hasP2pPermission()) {
            return
        }

        manager.cancelConnect(channel, object : WifiP2pManager.ActionListener {
            override fun onSuccess() {
                Log.d(TAG, "Wi-Fi Direct connect canceled after error")
            }

            override fun onFailure(reason: Int) {
                Log.w(TAG, "Wi-Fi Direct cancel connect failed: $reason")
            }
        })

        manager.removeGroup(channel, object : WifiP2pManager.ActionListener {
            override fun onSuccess() {
                Log.d(TAG, "Wi-Fi Direct group removed after error")
            }

            override fun onFailure(reason: Int) {
                Log.w(TAG, "Wi-Fi Direct remove group failed: $reason")
            }
        })
    }

    private fun recoverFromGroupError() {
        val manager = wifiP2pManager ?: return
        val channel = p2pChannel ?: return
        if (!hasP2pPermission()) {
            return
        }

        manager.removeGroup(channel, object : WifiP2pManager.ActionListener {
            override fun onSuccess() {
                Log.d(TAG, "Wi-Fi Direct group removed after create error")
                scheduleGroupCreateRetry()
            }

            override fun onFailure(reason: Int) {
                Log.w(TAG, "Wi-Fi Direct remove group failed after create error: $reason")
                scheduleGroupCreateRetry()
            }
        })
    }

    private fun scheduleGroupCreateRetry() {
        handler.removeCallbacks(retryGroupCreateRunnable)
        val attempts = groupCreateRetryAttempts.coerceAtMost(5)
        val delay = GROUP_RECOVERY_DELAY_MS * (attempts + 1)
        handler.postDelayed(retryGroupCreateRunnable, delay)
    }

    private fun createGroupIfNeeded(force: Boolean) {
        val manager = wifiP2pManager ?: return
        val channel = p2pChannel ?: return
        if (!hasP2pPermission()) {
            return
        }
        if (p2pActionInFlight) {
            return
        }

        val now = SystemClock.elapsedRealtime()
        if (!force && now - lastGroupCreateAttempt < GROUP_CREATE_COOLDOWN_MS) {
            return
        }
        lastGroupCreateAttempt = now
        p2pActionInFlight = true
        try {
            manager.requestGroupInfo(channel) { group ->
                if (group != null && group.isGroupOwner) {
                    Log.d(TAG, "Wi-Fi Direct group already active; skipping create")
                    p2pConnected = true
                    p2pIsGroupOwner = true
                    p2pActionInFlight = false
                    groupCreateRetryAttempts = 0
                    return@requestGroupInfo
                }

                val doCreate = {
                    try {
                        manager.createGroup(channel, object : WifiP2pManager.ActionListener {
                            override fun onSuccess() {
                                Log.d(TAG, "Wi-Fi Direct group creation requested")
                                p2pActionInFlight = false
                                groupCreateRetryAttempts = 0
                                discoverPeers(force = true)
                            }

                            override fun onFailure(reason: Int) {
                                p2pActionInFlight = false
                                groupCreateRetryAttempts += 1
                                handleP2pFailure("create group", reason)
                                pendingPreferGroupOwner = false
                            }
                        })
                    } catch (e: Exception) {
                        Log.w(TAG, "createGroup failed: ${e.message}")
                        p2pActionInFlight = false
                    }
                }

                try {
                    manager.stopPeerDiscovery(channel, object : WifiP2pManager.ActionListener {
                        override fun onSuccess() {
                            doCreate()
                        }

                        override fun onFailure(reason: Int) {
                            if (reason == WifiP2pManager.BUSY) {
                                Log.w(TAG, "Wi-Fi Direct stop discovery failed: BUSY")
                                p2pActionInFlight = false
                                groupCreateRetryAttempts += 1
                                scheduleGroupCreateRetry()
                                return
                            }
                            doCreate()
                        }
                    })
                } catch (e: Exception) {
                    Log.w(TAG, "stopPeerDiscovery failed: ${e.message}")
                    // Try creating anyway if stop failed (e.g. not discovering)
                    doCreate()
                }
            }
        } catch (e: Exception) {
            Log.w(TAG, "requestGroupInfo failed: ${e.message}")
            p2pActionInFlight = false
        }
    }

    private fun isLocationEnabled(): Boolean {
        val manager = context.getSystemService(Context.LOCATION_SERVICE) as LocationManager
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            manager.isLocationEnabled
        } else {
            val mode = try {
                Settings.Secure.getInt(
                    context.contentResolver,
                    Settings.Secure.LOCATION_MODE
                )
            } catch (_: Exception) {
                Settings.Secure.LOCATION_MODE_OFF
            }
            mode != Settings.Secure.LOCATION_MODE_OFF
        }
    }

    private fun buildPayload(local: DiscoveredDevice): ByteArray {
        val uuid = UUID.fromString(local.id)
        val uuidBytes = ByteBuffer.allocate(16)
            .putLong(uuid.mostSignificantBits)
            .putLong(uuid.leastSignificantBits)
            .array()

        val flags =
            ((PROTOCOL_VERSION and 0x0F) shl 4) or (deviceTypeCode(local.deviceType) and 0x0F)
        val portBytes = ByteBuffer.allocate(2).putShort(local.port.toShort()).array()
        val pairingFlag = if (SystemClock.elapsedRealtime() < pairingActiveUntil) 1 else 0
        val payload = ByteArray(1 + 2 + 16 + 1)
        var offset = 0
        payload[offset++] = flags.toByte()
        payload[offset++] = portBytes[0]
        payload[offset++] = portBytes[1]
        System.arraycopy(uuidBytes, 0, payload, offset, uuidBytes.size)
        offset += uuidBytes.size
        payload[offset] = pairingFlag.toByte()
        return payload
    }

    private fun buildNameData(local: DiscoveredDevice): ByteArray {
        val matchName = getMatchName(local)
        return trimUtf8Bytes(matchName, NAME_MAX_LEN)
    }

    private fun parsePayload(data: ByteArray, nameOverride: String?): ProximityPeer? {
        if (data.size < 1 + 2 + 16) {
            return null
        }

        val now = SystemClock.elapsedRealtime()
        if (pairingTargetId != null && now > pairingActiveUntil) {
            pairingTargetId = null
        }

        val flags = data[0].toInt() and 0xFF
        val protocol = (flags shr 4) and 0x0F
        if (protocol < MIN_COMPATIBLE_VERSION) {
            return null
        }

        val deviceTypeCode = flags and 0x0F
        val port = ByteBuffer.wrap(data, 1, 2).short.toInt() and 0xFFFF
        val uuidBytes = data.copyOfRange(3, 19)

        val uuidBuffer = ByteBuffer.wrap(uuidBytes)
        val uuid = UUID(uuidBuffer.long, uuidBuffer.long).toString()
        val matchName = nameOverride?.ifBlank { null } ?: "Unknown"

        val pairingIntent = if (protocol >= 3 && data.size >= 1 + 2 + 16 + 1) {
            data[19].toInt() != 0
        } else {
            false
        }
        val acceptPairIntent = pairingIntent && shouldAcceptPairIntent(uuid)

        if (protocol >= 2) {
            return ProximityPeer(
                deviceId = uuid,
                name = matchName,
                deviceType = deviceTypeString(deviceTypeCode),
                port = port,
                protocolVersion = protocol,
                matchName = matchName,
                pairingIntent = acceptPairIntent,
            )
        }

        if (data.size < 1 + 2 + 16 + 1 + 4) {
            return null
        }
        val nameLen = data[19].toInt() and 0xFF
        val nameStart = 20
        val nameEnd = nameStart + nameLen
        if (nameEnd + 4 > data.size) {
            return null
        }

        val legacyName = String(data, nameStart, nameLen, Charsets.UTF_8)
        val ipBytes = data.copyOfRange(nameEnd, nameEnd + 4)
        val ip = InetAddress.getByAddress(ipBytes).hostAddress

        return ProximityPeer(
            deviceId = uuid,
            name = legacyName.ifBlank { matchName },
            deviceType = deviceTypeString(deviceTypeCode),
            port = port,
            protocolVersion = protocol,
            ip = ip,
            matchName = matchName,
            pairingIntent = acceptPairIntent,
        )
    }

    private fun deviceTypeCode(type: String): Int {
        return when (type.lowercase()) {
            "android" -> 1
            "linux" -> 2
            "windows" -> 3
            "macos" -> 4
            else -> 0
        }
    }

    private fun deviceTypeString(code: Int): String {
        return when (code) {
            1 -> "android"
            2 -> "linux"
            3 -> "windows"
            4 -> "macos"
            else -> "unknown"
        }
    }

    private fun matchPeerByName(deviceName: String?): ProximityPeer? {
        if (deviceName.isNullOrBlank()) {
            return null
        }
        return peersById.values.firstOrNull { peer ->
            namesMatch(peer.matchName, deviceName)
        }
    }

    private fun namesMatch(pending: String?, candidate: String?): Boolean {
        if (pending.isNullOrBlank() || candidate.isNullOrBlank()) {
            return false
        }
        val normalizedPending = normalizeMatchName(pending)
        val normalizedCandidate = normalizeMatchName(candidate)
        if (normalizedPending.isEmpty() || normalizedCandidate.isEmpty()) {
            return false
        }
        return normalizedCandidate.contains(normalizedPending) ||
                normalizedPending.contains(normalizedCandidate)
    }

    private fun normalizeMatchName(name: String): String {
        val normalized = Normalizer.normalize(name, Normalizer.Form.NFKD)
        val noDiacritics = normalized.replace(Regex("\\p{M}+"), "")
        return noDiacritics
            .lowercase()
            .replace(Regex("[^\\p{L}\\p{N}]+"), "")
    }

    private fun shouldBeGroupOwner(peerId: String): Boolean {
        val localId = runCatching { getLocalDevice().id }.getOrNull() ?: return false
        return localId < peerId
    }

    private fun shouldAcceptPairIntent(peerId: String): Boolean {
        val target = pairingTargetId ?: return true
        return target == peerId
    }

    private fun getMatchName(local: DiscoveredDevice): String {
        val p2pName = localP2pDeviceName
        if (!p2pName.isNullOrBlank()) {
            return p2pName
        }
        val adapterName = runCatching { bluetoothAdapter?.name }.getOrNull()
        if (!adapterName.isNullOrBlank()) {
            return adapterName
        }
        val deviceName = runCatching {
            Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME)
        }.getOrNull()
        if (!deviceName.isNullOrBlank()) {
            return deviceName
        }
        return local.name
    }

    private fun trimUtf8Bytes(input: String, maxBytes: Int): ByteArray {
        if (maxBytes <= 0) {
            return ByteArray(0)
        }
        var count = 0
        val builder = StringBuilder()
        for (ch in input) {
            val bytes = ch.toString().toByteArray(Charsets.UTF_8)
            if (count + bytes.size > maxBytes) {
                break
            }
            builder.append(ch)
            count += bytes.size
        }
        return builder.toString().toByteArray(Charsets.UTF_8)
    }

    private fun readThisDeviceName(intent: Intent): String? {
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            intent.getParcelableExtra(
                WifiP2pManager.EXTRA_WIFI_P2P_DEVICE,
                WifiP2pDevice::class.java
            )?.deviceName
        } else {
            @Suppress("DEPRECATION")
            (intent.getParcelableExtra(WifiP2pManager.EXTRA_WIFI_P2P_DEVICE) as? WifiP2pDevice)
                ?.deviceName
        }
    }

    private fun isUsableIp(ip: String?): Boolean {
        if (ip.isNullOrBlank()) {
            return false
        }
        return ip != UNKNOWN_IP && !ip.startsWith(SYNTHETIC_IP_PREFIX)
    }

    private fun syntheticIpForDevice(deviceId: String): String {
        return try {
            val uuid = UUID.fromString(deviceId)
            val bytes = ByteBuffer.allocate(16)
                .putLong(uuid.mostSignificantBits)
                .putLong(uuid.leastSignificantBits)
                .array()
            val third = (bytes[14].toInt() and 0xFF) % 254 + 1
            val fourth = (bytes[15].toInt() and 0xFF) % 254 + 1
            "$SYNTHETIC_IP_PREFIX$third.$fourth"
        } catch (_: Exception) {
            UNKNOWN_IP
        }
    }

    private fun hasBleScanPermission(): Boolean {
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            ContextCompat.checkSelfPermission(
                context,
                android.Manifest.permission.BLUETOOTH_SCAN
            ) == PackageManager.PERMISSION_GRANTED
        } else {
            ContextCompat.checkSelfPermission(
                context,
                android.Manifest.permission.ACCESS_FINE_LOCATION
            ) == PackageManager.PERMISSION_GRANTED
        }
    }

    private fun hasBleAdvertisePermission(): Boolean {
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            ContextCompat.checkSelfPermission(
                context,
                android.Manifest.permission.BLUETOOTH_ADVERTISE
            ) == PackageManager.PERMISSION_GRANTED
        } else {
            true
        }
    }

    private fun hasP2pPermission(): Boolean {
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            ContextCompat.checkSelfPermission(
                context,
                android.Manifest.permission.NEARBY_WIFI_DEVICES
            ) == PackageManager.PERMISSION_GRANTED
        } else {
            ContextCompat.checkSelfPermission(
                context,
                android.Manifest.permission.ACCESS_FINE_LOCATION
            ) == PackageManager.PERMISSION_GRANTED
        }
    }
}
