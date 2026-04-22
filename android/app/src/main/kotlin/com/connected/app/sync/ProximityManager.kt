package com.connected.app.sync

import android.Manifest
import android.annotation.SuppressLint
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
import android.net.wifi.p2p.nsd.WifiP2pDnsSdServiceInfo
import android.net.wifi.p2p.nsd.WifiP2pDnsSdServiceRequest
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.provider.Settings
import android.util.Log
import androidx.annotation.RequiresPermission
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import androidx.core.location.LocationManagerCompat
import uniffi.connected_ffi.DiscoveredDevice
import uniffi.connected_ffi.getLocalDevice
import uniffi.connected_ffi.injectProximityDevice
import java.nio.ByteBuffer
import java.text.Normalizer
import java.util.Locale
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap

class ProximityManager(private val context: Context) {
    companion object {
        private const val TAG = "ProximityManager"

        // Wi-Fi Direct Bonjour/DNS-SD identity
        private const val SERVICE_INSTANCE_PREFIX = "connected"
        private const val SERVICE_TYPE = "_connected._udp"
        private const val TXT_KEY_APP = "app"
        private const val TXT_KEY_ID = "id"
        private const val TXT_KEY_NAME = "name"
        private const val TXT_KEY_TYPE = "type"
        private const val TXT_KEY_PORT = "port"
        private const val TXT_KEY_VERSION = "ver"
        private const val TXT_KEY_PAIR = "pair"
        private const val APP_ID = "connected"
        private const val PROTOCOL_VERSION = 1
        private const val MIN_COMPATIBLE_VERSION = 1

        private const val UNKNOWN_IP = "0.0.0.0"
        private const val SYNTHETIC_IP_PREFIX = "198.18."

        private const val DISCOVERY_COOLDOWN_MS = 4_000L
        private const val SERVICE_DISCOVERY_COOLDOWN_MS = 4_000L
        private const val DISCOVERY_LOOP_INTERVAL_MS = 7_500L
        private const val RETRY_DELAY_MS = 5_000L
        private const val CONNECT_RETRY_DELAY_MS = 2_000L
        private const val CONNECT_COOLDOWN_MS = 15_000L
        private const val GROUP_CREATE_COOLDOWN_MS = 15_000L
        private const val GROUP_RECOVERY_DELAY_MS = 2_000L
        private const val PEER_STALE_MS = 45_000L
        private const val SERVICE_STALE_MS = 45_000L

        private const val PAIR_INTENT_TTL_MS = 30_000L
        private const val PAIR_INTENT_DEBOUNCE_MS = 10_000L
    }

    private val wifiP2pManager =
        context.getSystemService(Context.WIFI_P2P_SERVICE) as WifiP2pManager?
    private var p2pChannel: WifiP2pManager.Channel? = null
    private var p2pReceiver: BroadcastReceiver? = null
    private val handler = Handler(Looper.getMainLooper())

    data class ProximityPeer(
        val deviceId: String,
        val name: String,
        val deviceType: String,
        val port: Int,
        val protocolVersion: Int,
        val ip: String? = null,
        val matchName: String = name,
        val pairingIntent: Boolean = false,
        val deviceAddress: String? = null,
        val lastSeenAtMs: Long = SystemClock.elapsedRealtime(),
    )

    private data class ServiceEntry(
        val deviceAddress: String,
        var deviceName: String? = null,
        var instanceName: String? = null,
        var txtRecord: Map<String, String>? = null,
        var lastSeenAtMs: Long = SystemClock.elapsedRealtime(),
    )

    private val peersById = ConcurrentHashMap<String, ProximityPeer>()
    private val serviceEntriesByAddress = ConcurrentHashMap<String, ServiceEntry>()
    private val p2pDevicesByAddress = ConcurrentHashMap<String, WifiP2pDevice>()
    private val p2pIpById = ConcurrentHashMap<String, String>()
    private val lastPairIntentHandledAt = ConcurrentHashMap<String, Long>()
    private val lastPeerLogAt = ConcurrentHashMap<String, Long>()

    private var serviceRequest: WifiP2pDnsSdServiceRequest? = null
    private var localServiceInfo: WifiP2pDnsSdServiceInfo? = null
    private var lastAdvertisedSignature: String? = null

    @Volatile
    private var pendingPeerId: String? = null

    @Volatile
    private var pendingPeerName: String? = null

    @Volatile
    private var pendingPreferGroupOwner = false

    @Volatile
    private var pairingActiveUntil = 0L

    @Volatile
    private var pairingTargetId: String? = null

    @Volatile
    private var p2pActionInFlight = false

    @Volatile
    private var p2pConnected = false

    @Volatile
    private var isGroupOwner = false

    @Volatile
    private var p2pEnabled = true

    private var lastConnectAttempt = 0L
    private var lastDiscoveryAttempt = 0L
    private var lastServiceDiscoveryAttempt = 0L
    private var lastGroupCreateAttempt = 0L
    private var localP2pDeviceName: String? = null
    private var groupCreateRetryAttempts = 0

    var onPairingIntent: ((String) -> Unit)? = null
    var hasIdeallyDiscoveredDevice: ((String) -> Boolean)? = null

    @SuppressLint("MissingPermission")
    private val retryDiscoveryRunnable = Runnable { discoverNearby(force = true) }

    @SuppressLint("MissingPermission")
    private val retryConnectRunnable = Runnable { discoverNearby(force = true) }

    @SuppressLint("MissingPermission")
    private val retryGroupCreateRunnable = Runnable { createGroupIfNeeded(force = true) }

    @SuppressLint("MissingPermission")
    private val discoveryLoopRunnable = object : Runnable {
        override fun run() {
            refreshLocalService()
            discoverNearby(force = true)
            cleanupStaleState()
            handler.postDelayed(this, DISCOVERY_LOOP_INTERVAL_MS)
        }
    }

    @RequiresPermission(
        anyOf = [
            Manifest.permission.ACCESS_FINE_LOCATION,
            Manifest.permission.NEARBY_WIFI_DEVICES,
        ],
    )
    fun start() {
        startWifiDirect()
    }

    @RequiresPermission(
        anyOf = [
            Manifest.permission.ACCESS_FINE_LOCATION,
            Manifest.permission.NEARBY_WIFI_DEVICES,
        ],
    )
    fun stop() {
        stopWifiDirect()
    }

    @RequiresPermission(
        anyOf = [
            Manifest.permission.ACCESS_FINE_LOCATION,
            Manifest.permission.NEARBY_WIFI_DEVICES,
        ],
    )
    fun requestConnect(deviceId: String) {
        val peer = peersById[deviceId]
        if (peer == null) {
            Log.d(TAG, "No proximity peer for device id $deviceId")
            discoverNearby(force = true)
            return
        }

        Log.d(TAG, "Proximity connect requested for ${peer.matchName} ($deviceId)")
        pairingActiveUntil = SystemClock.elapsedRealtime() + PAIR_INTENT_TTL_MS
        pairingTargetId = deviceId
        pendingPeerId = deviceId
        pendingPeerName = peer.matchName
        pendingPreferGroupOwner = shouldBeGroupOwner(deviceId)

        refreshLocalService(force = true)
        handler.postDelayed({ refreshLocalService(force = true) }, PAIR_INTENT_TTL_MS + 100)

        maybeConnectWifiDirect(peer, force = true)
        discoverNearby(force = true)
    }

    @RequiresPermission(
        anyOf = [
            Manifest.permission.ACCESS_FINE_LOCATION,
            Manifest.permission.NEARBY_WIFI_DEVICES,
        ],
    )
    private fun startWifiDirect() {
        stopWifiDirect()
        if (wifiP2pManager == null) {
            Log.w(TAG, "Wi-Fi Direct manager unavailable")
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
        setupDnsSdListeners()

        val filter = IntentFilter().apply {
            addAction(WifiP2pManager.WIFI_P2P_STATE_CHANGED_ACTION)
            addAction(WifiP2pManager.WIFI_P2P_PEERS_CHANGED_ACTION)
            addAction(WifiP2pManager.WIFI_P2P_CONNECTION_CHANGED_ACTION)
            addAction(WifiP2pManager.WIFI_P2P_THIS_DEVICE_CHANGED_ACTION)
            addAction(WifiP2pManager.WIFI_P2P_DISCOVERY_CHANGED_ACTION)
        }

        p2pReceiver = object : BroadcastReceiver() {
            @RequiresPermission(
                anyOf = [
                    Manifest.permission.ACCESS_FINE_LOCATION,
                    Manifest.permission.NEARBY_WIFI_DEVICES,
                ],
            )
            override fun onReceive(context: Context?, intent: Intent?) {
                val action = intent?.action ?: return
                when (action) {
                    WifiP2pManager.WIFI_P2P_STATE_CHANGED_ACTION -> {
                        val state = intent.getIntExtra(
                            WifiP2pManager.EXTRA_WIFI_STATE,
                            WifiP2pManager.WIFI_P2P_STATE_DISABLED,
                        )
                        p2pEnabled = state == WifiP2pManager.WIFI_P2P_STATE_ENABLED
                        if (p2pEnabled) {
                            requestLocalDeviceInfo()
                            refreshLocalService(force = true)
                            refreshServiceRequest(force = true)
                            discoverNearby(force = true)
                        } else {
                            p2pConnected = false
                            isGroupOwner = false
                            p2pIpById.clear()
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
                            refreshLocalService()
                        }
                    }

                    WifiP2pManager.WIFI_P2P_DISCOVERY_CHANGED_ACTION -> {
                        discoverNearby(force = true)
                    }
                }
            }
        }

        context.registerReceiver(p2pReceiver, filter)
        requestLocalDeviceInfo()
        refreshLocalService(force = true)
        refreshServiceRequest(force = true)
        discoverNearby(force = true)
        scheduleDiscoveryLoop()
    }

    private fun stopWifiDirect() {
        handler.removeCallbacks(discoveryLoopRunnable)
        handler.removeCallbacks(retryDiscoveryRunnable)
        handler.removeCallbacks(retryConnectRunnable)
        handler.removeCallbacks(retryGroupCreateRunnable)

        val manager = wifiP2pManager
        val channel = p2pChannel
        if (manager != null && channel != null && hasP2pPermission()) {
            runCatching {
                manager.stopPeerDiscovery(channel, object : WifiP2pManager.ActionListener {
                    override fun onSuccess() {}
                    override fun onFailure(reason: Int) {
                        Log.d(TAG, "stopPeerDiscovery failed on stop: $reason")
                    }
                })
            }

            runCatching {
                manager.clearServiceRequests(channel, object : WifiP2pManager.ActionListener {
                    override fun onSuccess() {}
                    override fun onFailure(reason: Int) {
                        Log.d(TAG, "clearServiceRequests failed on stop: $reason")
                    }
                })
            }

            runCatching {
                manager.clearLocalServices(channel, object : WifiP2pManager.ActionListener {
                    override fun onSuccess() {}
                    override fun onFailure(reason: Int) {
                        Log.d(TAG, "clearLocalServices failed on stop: $reason")
                    }
                })
            }
        }

        p2pReceiver?.let { receiver ->
            runCatching { context.unregisterReceiver(receiver) }
        }
        p2pReceiver = null
        p2pChannel = null
        serviceRequest = null
        localServiceInfo = null
        lastAdvertisedSignature = null

        p2pConnected = false
        isGroupOwner = false
        p2pEnabled = true
        p2pActionInFlight = false

        pendingPeerId = null
        pendingPeerName = null
        pendingPreferGroupOwner = false

        pairingTargetId = null
        pairingActiveUntil = 0L

        peersById.clear()
        p2pIpById.clear()
        p2pDevicesByAddress.clear()
        serviceEntriesByAddress.clear()
        lastPairIntentHandledAt.clear()
        lastPeerLogAt.clear()
    }

    private fun scheduleDiscoveryLoop() {
        handler.removeCallbacks(discoveryLoopRunnable)
        handler.postDelayed(discoveryLoopRunnable, DISCOVERY_LOOP_INTERVAL_MS)
    }

    private fun setupDnsSdListeners() {
        val manager = wifiP2pManager ?: return
        val channel = p2pChannel ?: return

        try {
            manager.setDnsSdResponseListeners(
                channel,
                WifiP2pManager.DnsSdServiceResponseListener { instanceName, registrationType, srcDevice ->
                    if (!registrationType.contains(SERVICE_TYPE)) {
                        return@DnsSdServiceResponseListener
                    }
                    upsertServiceEntry(srcDevice, instanceName, null)
                },
                WifiP2pManager.DnsSdTxtRecordListener { fullDomainName, txtRecordMap, srcDevice ->
                    if (!fullDomainName.contains(SERVICE_TYPE)) {
                        return@DnsSdTxtRecordListener
                    }
                    upsertServiceEntry(srcDevice, null, txtRecordMap)
                },
            )
        } catch (e: Exception) {
            Log.w(TAG, "Failed to set DNS-SD listeners: ${e.message}")
        }
    }

    private fun refreshLocalService(force: Boolean = false) {
        val manager = wifiP2pManager ?: return
        val channel = p2pChannel ?: return
        if (!hasP2pPermission()) {
            return
        }

        val local = runCatching { getLocalDevice() }.getOrNull() ?: return
        val pairingFlag = if (SystemClock.elapsedRealtime() < pairingActiveUntil) 1 else 0
        val matchName = getMatchName(local)
        val signature =
            "${local.id}|${local.name}|${local.deviceType}|${local.port}|$pairingFlag|$matchName"

        if (!force && signature == lastAdvertisedSignature && localServiceInfo != null) {
            return
        }

        val txtRecord = hashMapOf(
            TXT_KEY_APP to APP_ID,
            TXT_KEY_ID to local.id,
            TXT_KEY_NAME to trimTxtValue(local.name, 48),
            TXT_KEY_TYPE to normalizeDeviceType(local.deviceType),
            TXT_KEY_PORT to local.port.toString(),
            TXT_KEY_VERSION to PROTOCOL_VERSION.toString(),
            TXT_KEY_PAIR to pairingFlag.toString(),
        )

        val instanceName = safeInstanceName(local.id)
        val serviceInfo = WifiP2pDnsSdServiceInfo.newInstance(instanceName, SERVICE_TYPE, txtRecord)
        clearAndAddLocalService(manager, channel, serviceInfo, signature)
    }

    private fun clearAndAddLocalService(
        manager: WifiP2pManager,
        channel: WifiP2pManager.Channel,
        serviceInfo: WifiP2pDnsSdServiceInfo,
        signature: String,
    ) {
        try {
            manager.clearLocalServices(channel, object : WifiP2pManager.ActionListener {
                override fun onSuccess() {
                    addLocalService(manager, channel, serviceInfo, signature)
                }

                override fun onFailure(reason: Int) {
                    Log.w(TAG, "clearLocalServices failed: $reason")
                    addLocalService(manager, channel, serviceInfo, signature)
                }
            })
        } catch (e: Exception) {
            Log.w(TAG, "clearLocalServices threw: ${e.message}")
            addLocalService(manager, channel, serviceInfo, signature)
        }
    }

    private fun addLocalService(
        manager: WifiP2pManager,
        channel: WifiP2pManager.Channel,
        serviceInfo: WifiP2pDnsSdServiceInfo,
        signature: String,
    ) {
        try {
            manager.addLocalService(channel, serviceInfo, object : WifiP2pManager.ActionListener {
                override fun onSuccess() {
                    localServiceInfo = serviceInfo
                    lastAdvertisedSignature = signature
                    Log.d(TAG, "Wi-Fi Direct local DNS-SD service published")
                }

                override fun onFailure(reason: Int) {
                    Log.w(TAG, "addLocalService failed: $reason")
                }
            })
        } catch (e: Exception) {
            Log.w(TAG, "addLocalService threw: ${e.message}")
        }
    }

    private fun refreshServiceRequest(force: Boolean = false) {
        val manager = wifiP2pManager ?: return
        val channel = p2pChannel ?: return
        if (!hasP2pPermission()) {
            return
        }

        if (!force && serviceRequest != null) {
            return
        }

        val request = WifiP2pDnsSdServiceRequest.newInstance()
        serviceRequest = request

        try {
            manager.clearServiceRequests(channel, object : WifiP2pManager.ActionListener {
                override fun onSuccess() {
                    addServiceRequest(manager, channel, request)
                }

                override fun onFailure(reason: Int) {
                    Log.w(TAG, "clearServiceRequests failed: $reason")
                    addServiceRequest(manager, channel, request)
                }
            })
        } catch (e: Exception) {
            Log.w(TAG, "clearServiceRequests threw: ${e.message}")
            addServiceRequest(manager, channel, request)
        }
    }

    private fun addServiceRequest(
        manager: WifiP2pManager,
        channel: WifiP2pManager.Channel,
        request: WifiP2pDnsSdServiceRequest,
    ) {
        try {
            manager.addServiceRequest(channel, request, object : WifiP2pManager.ActionListener {
                @SuppressLint("MissingPermission")
                override fun onSuccess() {
                    discoverServices(force = true)
                }

                override fun onFailure(reason: Int) {
                    Log.w(TAG, "addServiceRequest failed: $reason")
                }
            })
        } catch (e: Exception) {
            Log.w(TAG, "addServiceRequest threw: ${e.message}")
        }
    }

    @RequiresPermission(
        anyOf = [
            Manifest.permission.ACCESS_FINE_LOCATION,
            Manifest.permission.NEARBY_WIFI_DEVICES,
        ],
    )
    private fun discoverNearby(force: Boolean = false) {
        val manager = wifiP2pManager ?: return
        val channel = p2pChannel ?: return
        if (!hasP2pPermission()) {
            Log.w(TAG, "Missing Wi-Fi Direct permissions")
            return
        }
        if (!p2pEnabled) {
            Log.d(TAG, "Wi-Fi Direct not enabled; skipping discovery")
            return
        }

        val now = SystemClock.elapsedRealtime()
        if (!force && now - lastDiscoveryAttempt < DISCOVERY_COOLDOWN_MS) {
            return
        }
        lastDiscoveryAttempt = now

        refreshServiceRequest()

        try {
            manager.discoverPeers(channel, object : WifiP2pManager.ActionListener {
                @SuppressLint("MissingPermission")
                override fun onSuccess() {
                    handler.postDelayed({ requestPeers() }, 1_000L)
                    discoverServices(force = true)
                }

                @SuppressLint("MissingPermission")
                override fun onFailure(reason: Int) {
                    handleP2pFailure("peer discovery", reason)
                    discoverServices(force = true)
                }
            })
        } catch (e: Exception) {
            Log.w(TAG, "discoverPeers failed: ${e.message}")
        }
    }

    @RequiresPermission(
        anyOf = [
            Manifest.permission.ACCESS_FINE_LOCATION,
            Manifest.permission.NEARBY_WIFI_DEVICES,
        ],
    )
    private fun discoverServices(force: Boolean = false) {
        val manager = wifiP2pManager ?: return
        val channel = p2pChannel ?: return
        if (!hasP2pPermission()) {
            return
        }
        if (!p2pEnabled) {
            return
        }
        if (serviceRequest == null) {
            refreshServiceRequest(force = true)
        }

        val now = SystemClock.elapsedRealtime()
        if (!force && now - lastServiceDiscoveryAttempt < SERVICE_DISCOVERY_COOLDOWN_MS) {
            return
        }
        lastServiceDiscoveryAttempt = now

        try {
            manager.discoverServices(channel, object : WifiP2pManager.ActionListener {
                override fun onSuccess() {
                    Log.d(TAG, "Wi-Fi Direct DNS-SD discovery started")
                }

                override fun onFailure(reason: Int) {
                    handleP2pFailure("service discovery", reason)
                }
            })
        } catch (e: Exception) {
            Log.w(TAG, "discoverServices failed: ${e.message}")
        }
    }

    private fun cleanupStaleState() {
        val now = SystemClock.elapsedRealtime()

        val staleServiceAddresses = mutableListOf<String>()
        for ((deviceAddress, entry) in serviceEntriesByAddress) {
            if (now - entry.lastSeenAtMs > SERVICE_STALE_MS) {
                staleServiceAddresses.add(deviceAddress)
            }
        }
        staleServiceAddresses.forEach { serviceEntriesByAddress.remove(it) }

        val stalePeerIds = mutableListOf<String>()
        for ((deviceId, peer) in peersById) {
            if (now - peer.lastSeenAtMs > PEER_STALE_MS) {
                stalePeerIds.add(deviceId)
            }
        }

        for (deviceId in stalePeerIds) {
            peersById.remove(deviceId)
            p2pIpById.remove(deviceId)
            lastPairIntentHandledAt.remove(deviceId)
            lastPeerLogAt.remove(deviceId)
            if (pendingPeerId == deviceId) {
                pendingPeerId = null
                pendingPeerName = null
            }
            if (pairingTargetId == deviceId) {
                pairingTargetId = null
            }
        }
    }

    private fun upsertServiceEntry(
        srcDevice: WifiP2pDevice?,
        instanceName: String?,
        txtRecordMap: Map<String, String>?,
    ) {
        val deviceAddress = srcDevice?.deviceAddress?.ifBlank { null } ?: return
        val entry = serviceEntriesByAddress[deviceAddress] ?: ServiceEntry(deviceAddress)
        entry.lastSeenAtMs = SystemClock.elapsedRealtime()

        if (!srcDevice.deviceName.isNullOrBlank()) {
            entry.deviceName = srcDevice.deviceName
        }
        if (!instanceName.isNullOrBlank()) {
            entry.instanceName = instanceName
        }
        if (txtRecordMap != null && txtRecordMap.isNotEmpty()) {
            entry.txtRecord = HashMap(txtRecordMap)
        }

        serviceEntriesByAddress[deviceAddress] = entry
        processServiceEntry(entry)
    }

    @SuppressLint("MissingPermission")
    private fun processServiceEntry(entry: ServiceEntry) {
        val peer = parsePeerFromService(entry) ?: return
        val localId = runCatching { getLocalDevice().id }.getOrNull()
        if (peer.deviceId == localId) {
            return
        }

        val now = SystemClock.elapsedRealtime()
        val previous = peersById[peer.deviceId]
        val mergedPeer = if (previous == null) {
            peer.copy(lastSeenAtMs = now)
        } else {
            peer.copy(
                ip = peer.ip ?: previous.ip,
                matchName = if (peer.matchName.isBlank()) previous.matchName else peer.matchName,
                deviceAddress = peer.deviceAddress ?: previous.deviceAddress,
                lastSeenAtMs = now,
            )
        }

        peersById[mergedPeer.deviceId] = mergedPeer

        val lastLog = lastPeerLogAt[mergedPeer.deviceId] ?: 0L
        if (now - lastLog > 2_000L) {
            Log.d(TAG, "Nearby peer: ${mergedPeer.matchName} (id=${mergedPeer.deviceId})")
            lastPeerLogAt[mergedPeer.deviceId] = now
        }

        if (mergedPeer.pairingIntent) {
            handlePairIntent(mergedPeer)
        }

        injectPeer(mergedPeer)

        val targetId = pendingPeerId ?: pairingTargetId
        if (targetId == mergedPeer.deviceId) {
            maybeConnectWifiDirect(mergedPeer, force = true)
        }
    }

    private fun parsePeerFromService(entry: ServiceEntry): ProximityPeer? {
        val txt = entry.txtRecord ?: return null

        val app = txt[TXT_KEY_APP]
        if (!app.isNullOrBlank() && !app.equals(APP_ID, ignoreCase = true)) {
            return null
        }

        val deviceId = txt[TXT_KEY_ID]?.trim().orEmpty()
        if (deviceId.isEmpty()) {
            return null
        }

        val protocolVersion = txt[TXT_KEY_VERSION]?.toIntOrNull() ?: 1
        if (protocolVersion < MIN_COMPATIBLE_VERSION) {
            return null
        }

        val port = txt[TXT_KEY_PORT]?.toIntOrNull() ?: return null
        if (port !in 1..65535) {
            return null
        }

        val candidateName = txt[TXT_KEY_NAME]
            ?.trim()
            ?.ifBlank { null }
            ?: entry.deviceName
            ?.trim()
            ?.ifBlank { null }
            ?: entry.instanceName
            ?.trim()
            ?.ifBlank { null }
            ?: "Unknown"

        val pairingIntentRaw = txt[TXT_KEY_PAIR] == "1"
        val pairingIntent = pairingIntentRaw && shouldAcceptPairIntent(deviceId)

        return ProximityPeer(
            deviceId = deviceId,
            name = candidateName,
            deviceType = normalizeDeviceType(txt[TXT_KEY_TYPE]),
            port = port,
            protocolVersion = protocolVersion,
            ip = null,
            matchName = candidateName,
            pairingIntent = pairingIntent,
            deviceAddress = entry.deviceAddress,
        )
    }

    private fun normalizeDeviceType(raw: String?): String {
        return when (raw?.lowercase(Locale.US)) {
            "android" -> "android"
            "linux" -> "linux"
            "windows" -> "windows"
            "macos" -> "macos"
            else -> "unknown"
        }
    }

    @SuppressLint("MissingPermission")
    private fun handlePairIntent(peer: ProximityPeer) {
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

    private fun injectPeer(peer: ProximityPeer) {
        val p2pIpOverride = if (p2pConnected) p2pIpById[peer.deviceId] else null
        val hasGoodIp = hasIdeallyDiscoveredDevice?.invoke(peer.deviceId) == true
        val ipForInject = when {
            isUsableIp(peer.ip) -> peer.ip!!
            isUsableIp(p2pIpOverride) -> p2pIpOverride!!
            hasGoodIp -> UNKNOWN_IP
            p2pConnected && isGroupOwner -> UNKNOWN_IP
            else -> syntheticIpForDevice(peer.deviceId)
        }

        try {
            injectProximityDevice(
                peer.deviceId,
                peer.name,
                peer.deviceType,
                ipForInject,
                peer.port.toUShort(),
            )
        } catch (e: Exception) {
            Log.w(TAG, "Failed to inject proximity device", e)
        }
    }

    @RequiresPermission(
        anyOf = [
            Manifest.permission.ACCESS_FINE_LOCATION,
            Manifest.permission.NEARBY_WIFI_DEVICES,
        ],
    )
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
        p2pDevicesByAddress.clear()
        candidates.forEach { candidate ->
            val address = candidate.deviceAddress
            if (!address.isNullOrBlank()) {
                p2pDevicesByAddress[address] = candidate
            }
        }

        if (p2pConnected) {
            return
        }

        if (pendingPeerId == null && pairingTargetId != null) {
            pendingPeerId = pairingTargetId
            pendingPeerName = pendingPeerId?.let { peersById[it]?.matchName }
            pendingPreferGroupOwner = pendingPeerId?.let { shouldBeGroupOwner(it) } == true
        }

        val targetId = pendingPeerId ?: pairingTargetId ?: return
        val targetPeer = peersById[targetId]
        val targetName = pendingPeerName ?: targetPeer?.matchName

        val byAddress = targetPeer?.deviceAddress?.let { p2pDevicesByAddress[it] }
        val byName = if (byAddress == null) {
            candidates.firstOrNull { namesMatch(targetName, it.deviceName) }
        } else {
            null
        }

        val candidate = byAddress ?: byName ?: if (candidates.size == 1) candidates.first() else null

        if (pendingPreferGroupOwner) {
            Log.d(TAG, "Acting as group owner; waiting for peer to connect")
            return
        }

        if (candidate == null) {
            Log.d(TAG, "No Wi-Fi Direct peer matched '$targetName' (candidates=${candidates.size})")
            scheduleDiscoveryRetry()
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

        val now = SystemClock.elapsedRealtime()
        if (now - lastConnectAttempt < CONNECT_COOLDOWN_MS) {
            Log.d(TAG, "Wi-Fi Direct connect cooldown active; skipping connect")
            return
        }
        lastConnectAttempt = now
        p2pActionInFlight = true

        val config = WifiP2pConfig().apply {
            deviceAddress = device.deviceAddress
            wps.setup = WpsInfo.PBC
            groupOwnerIntent = 0
        }

        val canConnect = ActivityCompat.checkSelfPermission(
            context,
            Manifest.permission.NEARBY_WIFI_DEVICES,
        ) == PackageManager.PERMISSION_GRANTED || ActivityCompat.checkSelfPermission(
            context,
            Manifest.permission.ACCESS_FINE_LOCATION,
        ) == PackageManager.PERMISSION_GRANTED

        if (!canConnect) {
            p2pActionInFlight = false
            Log.w(TAG, "Cannot connect: missing Wi-Fi Direct permissions")
            return
        }

        Log.d(TAG, "Wi-Fi Direct connect to ${device.deviceName} (${device.deviceAddress})")
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

    private fun handleConnectionChanged() {
        val manager = wifiP2pManager ?: return
        val channel = p2pChannel ?: return
        manager.requestConnectionInfo(channel) { info ->
            p2pConnected = info.groupFormed
            isGroupOwner = info.isGroupOwner

            if (info.groupFormed) {
                refreshLocalService(force = true)

                if (!info.isGroupOwner) {
                    val peerId = pendingPeerId
                    val peer = peerId?.let { peersById[it] }
                    val groupOwnerIp = info.groupOwnerAddress?.hostAddress
                    if (peer != null && !groupOwnerIp.isNullOrEmpty()) {
                        p2pIpById[peer.deviceId] = groupOwnerIp
                        injectPeer(peer.copy(ip = groupOwnerIp, lastSeenAtMs = SystemClock.elapsedRealtime()))
                    }
                }
            } else {
                p2pIpById.clear()
                refreshLocalService(force = true)
                discoverNearby(force = true)
            }
        }
    }

    @Suppress("SameParameterValue")
    @RequiresPermission(
        anyOf = [
            Manifest.permission.ACCESS_FINE_LOCATION,
            Manifest.permission.NEARBY_WIFI_DEVICES,
        ],
    )
    private fun maybeConnectWifiDirect(peer: ProximityPeer, force: Boolean) {
        if (wifiP2pManager == null || !hasP2pPermission() || !p2pEnabled) {
            Log.d(
                TAG,
                "Wi-Fi Direct connect skipped (manager=${wifiP2pManager != null}, connected=$p2pConnected, permission=${hasP2pPermission()}, enabled=$p2pEnabled)",
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

        if (p2pConnected) {
            Log.d(TAG, "Wi-Fi Direct already connected; skipping connect")
            return
        }

        if (pendingPreferGroupOwner) {
            createGroupIfNeeded(force)
        } else {
            discoverNearby(force = force)
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

        if (
            reason == WifiP2pManager.BUSY ||
            reason == WifiP2pManager.ERROR ||
            (action == "connect" && pendingPeerId != null)
        ) {
            scheduleDiscoveryRetry(force = true)
        }

        if (action == "connect" && pendingPeerId != null) {
            if (reason == WifiP2pManager.ERROR) {
                recoverFromConnectError()
            }
            scheduleConnectRetry()
        }

        if (
            action == "create group" &&
            (reason == WifiP2pManager.BUSY || reason == WifiP2pManager.ERROR)
        ) {
            recoverFromGroupError()
        }
    }

    private fun scheduleDiscoveryRetry(force: Boolean = false) {
        handler.removeCallbacks(retryDiscoveryRunnable)
        handler.postDelayed(
            retryDiscoveryRunnable,
            if (force) CONNECT_RETRY_DELAY_MS else RETRY_DELAY_MS,
        )
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

    @RequiresPermission(
        anyOf = [
            Manifest.permission.ACCESS_FINE_LOCATION,
            Manifest.permission.NEARBY_WIFI_DEVICES,
        ],
    )
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
                    isGroupOwner = true
                    p2pActionInFlight = false
                    groupCreateRetryAttempts = 0
                    return@requestGroupInfo
                }

                val doCreate = {
                    try {
                        manager.createGroup(channel, object : WifiP2pManager.ActionListener {
                            @SuppressLint("MissingPermission")
                            override fun onSuccess() {
                                Log.d(TAG, "Wi-Fi Direct group creation requested")
                                p2pActionInFlight = false
                                groupCreateRetryAttempts = 0
                                discoverNearby(force = true)
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
                    doCreate()
                }
            }
        } catch (e: Exception) {
            Log.w(TAG, "requestGroupInfo failed: ${e.message}")
            p2pActionInFlight = false
        }
    }

    @RequiresPermission(
        allOf = [
            Manifest.permission.ACCESS_FINE_LOCATION,
            Manifest.permission.NEARBY_WIFI_DEVICES,
        ],
    )
    private fun requestLocalDeviceInfo() {
        val manager = wifiP2pManager ?: return
        val channel = p2pChannel ?: return
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                manager.requestDeviceInfo(channel) { device ->
                    if (device != null && !device.deviceName.isNullOrBlank()) {
                        localP2pDeviceName = device.deviceName
                        Log.d(TAG, "Wi-Fi Direct local name (request): ${device.deviceName}")
                    }
                }
            }
        } catch (e: Exception) {
            Log.w(TAG, "requestDeviceInfo failed: ${e.message}")
        }
    }

    private fun trimTxtValue(value: String, maxLen: Int): String {
        if (maxLen <= 0) {
            return ""
        }
        return value.take(maxLen)
    }

    private fun safeInstanceName(deviceId: String): String {
        val seed = deviceId.take(8).lowercase(Locale.US)
        val cleanedSeed = seed.replace(Regex("[^a-z0-9]"), "")
        val suffix = cleanedSeed.ifEmpty { "device" }
        return "$SERVICE_INSTANCE_PREFIX-$suffix"
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
            .lowercase(Locale.US)
            .replace(Regex("[^\\p{L}\\p{N}]+"), "")
    }

    private fun shouldBeGroupOwner(peerId: String): Boolean {
        val localId = runCatching { getLocalDevice().id }.getOrNull() ?: return false
        return localId < peerId
    }

    private fun shouldAcceptPairIntent(peerId: String): Boolean {
        val now = SystemClock.elapsedRealtime()
        if (pairingTargetId != null && now > pairingActiveUntil) {
            pairingTargetId = null
        }
        val target = pairingTargetId ?: return true
        return target == peerId
    }

    private fun getMatchName(local: DiscoveredDevice): String {
        val p2pName = localP2pDeviceName
        if (!p2pName.isNullOrBlank()) {
            return p2pName
        }
        val deviceName = runCatching {
            Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME)
        }.getOrNull()
        if (!deviceName.isNullOrBlank()) {
            return deviceName
        }
        return local.name
    }

    private fun readThisDeviceName(intent: Intent): String? {
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            intent.getParcelableExtra(
                WifiP2pManager.EXTRA_WIFI_P2P_DEVICE,
                WifiP2pDevice::class.java,
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

    private fun hasP2pPermission(): Boolean {
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            ContextCompat.checkSelfPermission(
                context,
                Manifest.permission.NEARBY_WIFI_DEVICES,
            ) == PackageManager.PERMISSION_GRANTED
        } else {
            ContextCompat.checkSelfPermission(
                context,
                Manifest.permission.ACCESS_FINE_LOCATION,
            ) == PackageManager.PERMISSION_GRANTED
        }
    }

    private fun isLocationEnabled(): Boolean {
        val manager = context.getSystemService(Context.LOCATION_SERVICE) as LocationManager
        return LocationManagerCompat.isLocationEnabled(manager)
    }
}
