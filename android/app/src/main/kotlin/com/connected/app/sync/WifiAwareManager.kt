package com.connected.app.sync

import android.Manifest
import android.annotation.SuppressLint
import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.net.NetworkSpecifier
import android.net.wifi.aware.AttachCallback
import android.net.wifi.aware.DiscoverySession
import android.net.wifi.aware.DiscoverySessionCallback
import android.net.wifi.aware.PeerHandle
import android.net.wifi.aware.PublishConfig
import android.net.wifi.aware.PublishDiscoverySession
import android.net.wifi.aware.SubscribeConfig
import android.net.wifi.aware.SubscribeDiscoverySession
import android.net.wifi.aware.WifiAwareNetworkInfo
import android.net.wifi.aware.WifiAwareNetworkSpecifier
import android.net.wifi.aware.WifiAwareSession
import android.net.wifi.aware.WifiAwareManager as AndroidWifiAwareManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.ParcelFileDescriptor
import android.os.SystemClock
import android.util.Log
import androidx.annotation.RequiresApi
import androidx.annotation.RequiresPermission
import uniffi.connected_ffi.DiscoveredDevice
import uniffi.connected_ffi.getLocalDevice
import uniffi.connected_ffi.injectAwareSocket
import uniffi.connected_ffi.injectProximityDevice
import java.net.DatagramSocket
import java.net.InetSocketAddress
import java.nio.ByteBuffer
import java.nio.charset.StandardCharsets
import java.util.Base64
import java.util.concurrent.ConcurrentHashMap
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

@RequiresApi(29)
class ConnectedWifiAwareManager(private val context: Context) {

    companion object {
        private const val TAG = "WifiAwareManager"
        private const val SERVICE_TYPE = "_connected._udp"
        private const val SERVICE_INSTANCE_PREFIX = "conn"
        private const val PROTOCOL_VERSION = 1
        private const val MIN_COMPATIBLE_VERSION = 1
        private const val MSG_KEY_DEVICE_ID = "id"
        private const val MSG_KEY_NAME = "name"
        private const val MSG_KEY_TYPE = "type"
        private const val MSG_KEY_PORT = "port"
        private const val MSG_KEY_VERSION = "ver"
        private const val MSG_KEY_PAIRING_NAME = "pairingName"
        private const val MSG_KEY_VENDOR_NAME = "vendorName"
        private const val MSG_KEY_MODEL_NAME = "modelName"

        private const val REATTACH_DELAY_MS = 5_000L
        private const val STALE_PEER_MS = 45_000L
        private const val CLEANUP_INTERVAL_MS = 5_000L
        private const val PAIR_INTENT_TTL_MS = 30_000L
        private const val NETWORK_REQUEST_COOLDOWN_MS = 20_000L
        private const val IPPROTO_UDP = 17

        fun isSupported(context: Context): Boolean {
            if (Build.VERSION.SDK_INT < 29) return false
            val manager = context.getSystemService(Context.WIFI_AWARE_SERVICE) as? AndroidWifiAwareManager
            return manager != null
        }
    }

    private data class AwarePeer(
        val deviceId: String,
        val name: String,
        val deviceType: String,
        val port: Int,
        val peerHandle: PeerHandle,
        val handleSource: PeerHandleSource,
        val psk: String,
        val lastSeenAtMs: Long = SystemClock.elapsedRealtime(),
    )

    private enum class PeerHandleSource {
        Publish,
        Subscribe,
    }

    private val awareManager = context.getSystemService(Context.WIFI_AWARE_SERVICE) as? AndroidWifiAwareManager
    private var session: WifiAwareSession? = null
    private var publishSession: PublishDiscoverySession? = null
    private var subscribeSession: SubscribeDiscoverySession? = null
    private val handler = Handler(Looper.getMainLooper())

    private val peers = ConcurrentHashMap<String, AwarePeer>()
    private val peersByHandle = ConcurrentHashMap<PeerHandle, String>()
    private val lastPeerLogAt = ConcurrentHashMap<String, Long>()
    private val networkCallbacks = ConcurrentHashMap<String, ConnectivityManager.NetworkCallback>()
    private val networkRequestStartedAt = ConcurrentHashMap<String, Long>()
    private val connectedPeers = ConcurrentHashMap.newKeySet<String>()
    private val connectedEndpoints = ConcurrentHashMap<String, Pair<String, Int>>()

    @Volatile private var pendingPeerId: String? = null
    @Volatile private var pairingActiveUntil = 0L
    @Volatile private var pairingTargetId: String? = null
    @Volatile private var lastAttachAttempt = 0L
    @Volatile private var attachRetryScheduled = false
    @Volatile private var ignoredPublishTerminations = 0
    @Volatile private var ignoredSubscribeTerminations = 0

    var onPairingIntent: ((String) -> Unit)? = null
    var onAwareConnected: ((peerId: String, ipv6: String, port: Int) -> Unit)? = null
    var onAwareLost: ((peerId: String) -> Unit)? = null
    private val attachCallback = object : AttachCallback() {
        override fun onAttachFailed() {
            scheduleAttachRetry("WiFi Aware attach failed, retrying...")
        }

        override fun onAttached(wifiAwareSession: WifiAwareSession) {
            Log.d(TAG, "WiFi Aware attached")
            attachRetryScheduled = false
            session = wifiAwareSession
            startPublishSubscribe()
        }
    }

    private val publishCallback = object : DiscoverySessionCallback() {
        override fun onPublishStarted(session: PublishDiscoverySession) {
            Log.d(TAG, "WiFi Aware publish started")
            clearPeersForSource(PeerHandleSource.Publish)
            publishSession = session
        }

        override fun onSessionConfigFailed() {
            Log.w(TAG, "WiFi Aware publish config failed")
            refreshPublish()
        }

        override fun onSessionTerminated() {
            Log.w(TAG, "WiFi Aware publish session terminated; refreshing")
            clearPeersForSource(PeerHandleSource.Publish)
            publishSession = null
            if (ignoredPublishTerminations > 0) {
                ignoredPublishTerminations -= 1
                return
            }
            refreshPublish()
        }

        override fun onMessageReceived(peerHandle: PeerHandle, message: ByteArray) {
            handlePeerMessage(peerHandle, message, PeerHandleSource.Publish)
        }
    }

    private val subscribeCallback = object : DiscoverySessionCallback() {
        override fun onSubscribeStarted(session: SubscribeDiscoverySession) {
            Log.d(TAG, "WiFi Aware subscribe started")
            clearPeersForSource(PeerHandleSource.Subscribe)
            subscribeSession = session
        }

        override fun onSessionConfigFailed() {
            Log.w(TAG, "WiFi Aware subscribe config failed")
            refreshSubscribe()
        }

        override fun onSessionTerminated() {
            Log.w(TAG, "WiFi Aware subscribe session terminated; refreshing")
            clearPeersForSource(PeerHandleSource.Subscribe)
            subscribeSession = null
            if (ignoredSubscribeTerminations > 0) {
                ignoredSubscribeTerminations -= 1
                return
            }
            refreshSubscribe()
        }

        override fun onServiceDiscovered(
            peerHandle: PeerHandle,
            serviceSpecificInfo: ByteArray,
            matchFilter: List<ByteArray>
        ) {
            Log.d(TAG, "WiFi Aware service discovered")
            handleServiceDiscovered(peerHandle, serviceSpecificInfo)
        }

        override fun onServiceLost(peerHandle: PeerHandle, reason: Int) {
            val deviceId = peersByHandle.remove(peerHandle)
            if (deviceId != null) {
                val peer = peers[deviceId]
                if (peer?.peerHandle == peerHandle) {
                    peers.remove(deviceId)
                }
                Log.d(TAG, "WiFi Aware service lost: $deviceId (reason: $reason)")
            }
        }

        override fun onMessageReceived(peerHandle: PeerHandle, message: ByteArray) {
            handlePeerMessage(peerHandle, message, PeerHandleSource.Subscribe)
        }
    }

    @RequiresPermission(
        anyOf = [
            Manifest.permission.ACCESS_FINE_LOCATION,
            Manifest.permission.NEARBY_WIFI_DEVICES,
        ],
    )
    fun start() {
        attach()
        startCleanupLoop()
    }

    fun stop() {
        handler.removeCallbacksAndMessages(null)
        publishSession?.close()
        subscribeSession?.close()
        session?.close()
        unregisterAwareNetworks()
        publishSession = null
        subscribeSession = null
        session = null
        peers.clear()
        peersByHandle.clear()
        lastPeerLogAt.clear()
        networkRequestStartedAt.clear()
        connectedPeers.clear()
        connectedEndpoints.clear()
        attachRetryScheduled = false
        ignoredPublishTerminations = 0
        ignoredSubscribeTerminations = 0
        pendingPeerId = null
        pairingTargetId = null
        pairingActiveUntil = 0L
        Log.d(TAG, "WiFi Aware manager stopped")
    }

    @RequiresPermission(
        anyOf = [
            Manifest.permission.ACCESS_FINE_LOCATION,
            Manifest.permission.NEARBY_WIFI_DEVICES,
        ],
    )
    fun requestConnect(deviceId: String) {
        val peer = peers[deviceId]
        Log.d(TAG, "WiFi Aware connect requested for ${peer?.name ?: deviceId} ($deviceId)")

        pairingActiveUntil = SystemClock.elapsedRealtime() + PAIR_INTENT_TTL_MS
        pairingTargetId = deviceId
        pendingPeerId = deviceId

        if (peer == null) {
            Log.d(TAG, "No WiFi Aware peer for $deviceId yet; waiting for discovery")
            refreshPublish()
            refreshSubscribe()
            return
        }

        connectedEndpoints[deviceId]?.let { (ip, port) ->
            Log.d(TAG, "WiFi Aware endpoint already connected for $deviceId")
            handler.post {
                onAwareConnected?.invoke(deviceId, ip, port)
            }
            return
        }

        if (!isUsableForNetworkRequest(peer)) {
            Log.d(TAG, "Waiting for ${requestRoleName(peer)} session peer handle for $deviceId")
            refreshPublish()
            refreshSubscribe()
            return
        }

        maybeRequestAwareNetwork(peer)
    }

    fun isConnected(deviceId: String): Boolean = connectedEndpoints.containsKey(deviceId)

    fun connectedEndpoint(deviceId: String): Pair<String, Int>? = connectedEndpoints[deviceId]

    private fun clearPeersForSource(source: PeerHandleSource) {
        val stalePeers = peers.values.filter { it.handleSource == source }
        stalePeers.forEach { peer ->
            peers.remove(peer.deviceId, peer)
            peersByHandle.remove(peer.peerHandle)
        }
    }

    @SuppressLint("MissingPermission")
    private fun attach() {
        val manager = awareManager ?: run {
            Log.w(TAG, "WiFi Aware manager unavailable")
            return
        }
        if (!manager.isAvailable) {
            scheduleAttachRetry("WiFi Aware currently unavailable, retrying...")
            return
        }

        val now = SystemClock.elapsedRealtime()
        if (now - lastAttachAttempt < REATTACH_DELAY_MS) {
            scheduleAttachRetry("WiFi Aware attach throttled, retrying...")
            return
        }
        lastAttachAttempt = now

        session?.close()
        session = null
        try {
            manager.attach(attachCallback, handler)
        } catch (e: SecurityException) {
            Log.w(TAG, "WiFi Aware attach blocked by permission or Location state", e)
            scheduleAttachRetry("WiFi Aware attach blocked, retrying...")
        }
    }

    private fun scheduleAttachRetry(message: String) {
        if (attachRetryScheduled) return
        attachRetryScheduled = true
        Log.w(TAG, message)
        handler.postDelayed({
            attachRetryScheduled = false
            attach()
        }, REATTACH_DELAY_MS)
    }

    @SuppressLint("MissingPermission")
    private fun startPublishSubscribe() {
        val s = session ?: return
        refreshPublish()
        refreshSubscribe()
    }

    @SuppressLint("MissingPermission")
    private fun refreshPublish() {
        val s = session ?: return
        publishSession?.let {
            ignoredPublishTerminations += 1
            it.close()
            publishSession = null
        }

        val local = try {
            getLocalDevice()
        } catch (e: Exception) {
            Log.w(TAG, "Failed to get local device", e)
            return
        }

        val matchName = local.name

        val serviceInfo = buildServiceInfo(local.id, matchName, local.deviceType, local.port.toInt())
        val config = PublishConfig.Builder()
            .setServiceName(SERVICE_TYPE)
            .setPublishType(PublishConfig.PUBLISH_TYPE_SOLICITED)
            .setServiceSpecificInfo(serviceInfo)
            .build()

        try {
            s.publish(config, publishCallback, handler)
        } catch (e: SecurityException) {
            Log.w(TAG, "WiFi Aware publish blocked by permission or Location state", e)
        }
    }

    @SuppressLint("MissingPermission")
    private fun refreshSubscribe() {
        val s = session ?: return
        subscribeSession?.let {
            ignoredSubscribeTerminations += 1
            it.close()
            subscribeSession = null
        }

        val config = SubscribeConfig.Builder()
            .setServiceName(SERVICE_TYPE)
            .setSubscribeType(SubscribeConfig.SUBSCRIBE_TYPE_ACTIVE)
            .build()

        try {
            s.subscribe(config, subscribeCallback, handler)
        } catch (e: SecurityException) {
            Log.w(TAG, "WiFi Aware subscribe blocked by permission or Location state", e)
        }
    }

    private fun buildServiceInfo(
        deviceId: String,
        name: String,
        deviceType: String,
        port: Int,
    ): ByteArray {
        val entries = mapOf(
            MSG_KEY_DEVICE_ID to deviceId,
            MSG_KEY_NAME to name.take(48),
            MSG_KEY_TYPE to deviceType,
            MSG_KEY_PORT to port.toString(),
            MSG_KEY_VERSION to PROTOCOL_VERSION.toString(),
            MSG_KEY_PAIRING_NAME to name.take(48),
            MSG_KEY_VENDOR_NAME to "Connected",
            MSG_KEY_MODEL_NAME to deviceType.take(48),
        )
        return encodeTxtRecords(entries)
    }

    private fun encodeTxtRecords(entries: Map<String, String>): ByteArray {
        val buffer = ByteBuffer.allocate(1024)
        for ((key, value) in entries) {
            val kvBytes = "$key=$value".toByteArray(StandardCharsets.UTF_8)
            buffer.put((kvBytes.size and 0xFF).toByte())
            buffer.put(kvBytes)
        }
        buffer.flip()
        val result = ByteArray(buffer.remaining())
        buffer.get(result)
        return result
    }

    private fun decodeTxtRecords(data: ByteArray): Map<String, String> {
        val entries = mutableMapOf<String, String>()
        var offset = 0
        while (offset < data.size) {
            val len = data[offset].toInt() and 0xFF
            offset++
            if (offset + len > data.size) break
            val kvBytes = data.copyOfRange(offset, offset + len)
            offset += len
            val kv = String(kvBytes, StandardCharsets.UTF_8)
            val eqIdx = kv.indexOf('=')
            if (eqIdx > 0) {
                entries[kv.substring(0, eqIdx)] = kv.substring(eqIdx + 1)
            }
        }
        return entries
    }

    private fun handleServiceDiscovered(peerHandle: PeerHandle, serviceSpecificInfo: ByteArray) {
        val entries = decodeTxtRecords(serviceSpecificInfo)
        val peer = processDiscoveredPeer(peerHandle, PeerHandleSource.Subscribe, entries) ?: return
        if (shouldInitiate(peer.deviceId)) {
            sendLocalInfo(peerHandle)
            requestAwareNetworkSoon(peer.deviceId)
        }
    }

    private fun handlePeerMessage(peerHandle: PeerHandle, message: ByteArray, source: PeerHandleSource) {
        val entries = decodeTxtRecords(message)
        val peer = processDiscoveredPeer(peerHandle, source, entries) ?: return
        maybeRequestAwareNetwork(peer)
    }

    private fun processDiscoveredPeer(
        peerHandle: PeerHandle,
        source: PeerHandleSource,
        entries: Map<String, String>,
    ): AwarePeer? {
        val localId = try {
            getLocalDevice().id
        } catch (_: Exception) {
            return null
        }

        val deviceId = entries[MSG_KEY_DEVICE_ID]?.trim().orEmpty()
        if (deviceId.isEmpty() || deviceId == localId) return null

        val protocolVersion = entries[MSG_KEY_VERSION]?.toIntOrNull() ?: 1
        if (protocolVersion < MIN_COMPATIBLE_VERSION) return null

        val port = entries[MSG_KEY_PORT]?.toIntOrNull() ?: return null
        if (port !in 1..65535) return null

        val name = entries[MSG_KEY_NAME]?.trim()?.ifBlank { null } ?: "Unknown"
        val deviceType = entries[MSG_KEY_TYPE]?.trim()?.ifBlank { null } ?: "unknown"
        val psk = generatePairPsk(localId, deviceId)

        val now = SystemClock.elapsedRealtime()
        val existing = peers[deviceId]
        val keepExistingHandle =
            existing != null &&
                isUsableForNetworkRequest(existing) &&
                !isUsableForNetworkRequest(deviceId, source)
        val peer = if (keepExistingHandle) {
            existing.copy(
                name = name,
                deviceType = deviceType,
                port = port,
                psk = psk,
                lastSeenAtMs = now,
            )
        } else {
            AwarePeer(
                deviceId = deviceId,
                name = name,
                deviceType = deviceType,
                port = port,
                peerHandle = peerHandle,
                handleSource = source,
                psk = psk,
                lastSeenAtMs = now,
            )
        }

        peers[deviceId] = peer
        peersByHandle[peerHandle] = deviceId

        val lastLog = lastPeerLogAt[deviceId] ?: 0L
        if (now - lastLog > 2_000L) {
            Log.d(TAG, "WiFi Aware peer: $name ($deviceId)")
            lastPeerLogAt[deviceId] = now
        }

        injectPeer(peer)
        return peer
    }

    private fun injectPeer(peer: AwarePeer) {
        try {
            injectProximityDevice(
                peer.deviceId,
                peer.name,
                peer.deviceType,
                "0.0.0.0",
                peer.port.toUShort(),
            )
        } catch (e: Exception) {
            Log.w(TAG, "Failed to inject proximity device", e)
        }
    }

    @SuppressLint("MissingPermission")
    private fun maybeRequestAwareNetwork(peer: AwarePeer) {
        val isPublisher = !shouldInitiate(peer.deviceId)
        if (!isUsableForNetworkRequest(peer)) {
            Log.d(TAG, "Waiting for ${requestRoleName(peer)} session peer handle for ${peer.deviceId}")
            return
        }
        requestAwareNetwork(peer, isPublisher)
    }

    private fun isUsableForNetworkRequest(peer: AwarePeer): Boolean =
        isUsableForNetworkRequest(peer.deviceId, peer.handleSource)

    private fun isUsableForNetworkRequest(deviceId: String, source: PeerHandleSource): Boolean {
        val isPublisher = !shouldInitiate(deviceId)
        return if (isPublisher) {
            source == PeerHandleSource.Publish
        } else {
            source == PeerHandleSource.Subscribe
        }
    }

    private fun requestRoleName(peer: AwarePeer): String =
        if (!shouldInitiate(peer.deviceId)) "publish" else "subscribe"

    private fun requestAwareNetworkSoon(deviceId: String) {
        handler.postDelayed({
            val peer = peers[deviceId] ?: return@postDelayed
            maybeRequestAwareNetwork(peer)
        }, 1_500L)
    }

    @SuppressLint("MissingPermission")
    private fun requestAwareNetwork(peer: AwarePeer, isPublisher: Boolean) {
        if (connectedPeers.contains(peer.deviceId)) {
            Log.d(TAG, "WiFi Aware network already connected for ${peer.deviceId}")
            return
        }
        val key = "${peer.deviceId}:${if (isPublisher) "pub" else "sub"}"
        if (networkCallbacks.containsKey(key)) {
            Log.d(TAG, "WiFi Aware network request already active for ${peer.deviceId} ($key)")
            return
        }
        val now = SystemClock.elapsedRealtime()
        val lastStartedAt = networkRequestStartedAt[key] ?: 0L
        if (now - lastStartedAt < NETWORK_REQUEST_COOLDOWN_MS) {
            Log.d(TAG, "WiFi Aware network request cooldown active for ${peer.deviceId}")
            return
        }

        val discoverySession: DiscoverySession = if (isPublisher) {
            publishSession ?: run {
                Log.d(TAG, "WiFi Aware publish session not ready for ${peer.deviceId}")
                return
            }
        } else {
            subscribeSession ?: run {
                Log.d(TAG, "WiFi Aware subscribe session not ready for ${peer.deviceId}")
                return
            }
        }

        val specifier = buildNetworkSpecifier(discoverySession, peer, isPublisher)

        val request = NetworkRequest.Builder()
            .addTransportType(NetworkCapabilities.TRANSPORT_WIFI_AWARE)
            .setNetworkSpecifier(specifier)
            .build()

        val cm = context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
        val callback = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                Log.d(TAG, "WiFi Aware network available for ${peer.deviceId}")
            }

            override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) {
                val info = capabilities.transportInfo as? WifiAwareNetworkInfo ?: return
                handleAwareNetworkReady(network, info, peer)
            }

            override fun onLost(network: Network) {
                Log.d(TAG, "WiFi Aware network lost for ${peer.deviceId}")
                networkCallbacks.remove(key)
                networkRequestStartedAt.remove(key)
                connectedPeers.remove(peer.deviceId)
                connectedEndpoints.remove(peer.deviceId)
                handler.post { onAwareLost?.invoke(peer.deviceId) }
            }

            override fun onUnavailable() {
                Log.d(TAG, "WiFi Aware network unavailable for ${peer.deviceId}")
                networkCallbacks.remove(key)
                networkRequestStartedAt.remove(key)
                connectedPeers.remove(peer.deviceId)
                connectedEndpoints.remove(peer.deviceId)
                handler.post { onAwareLost?.invoke(peer.deviceId) }
            }
        }

        networkCallbacks[key] = callback
        networkRequestStartedAt[key] = now
        try {
            Log.d(
                TAG,
                "Requesting WiFi Aware network for ${peer.deviceId} as ${if (isPublisher) "publisher" else "subscriber"}",
            )
            cm.requestNetwork(request, callback, handler)
            Log.d(TAG, "WiFi Aware network request submitted for ${peer.deviceId} ($key)")
        } catch (e: Exception) {
            networkCallbacks.remove(key)
            networkRequestStartedAt.remove(key)
            Log.w(TAG, "WiFi Aware network request failed: ${e.message}", e)
        }
    }

    private fun buildNetworkSpecifier(
        discoverySession: DiscoverySession,
        peer: AwarePeer,
        isPublisher: Boolean,
    ): NetworkSpecifier {
        val builder = WifiAwareNetworkSpecifier.Builder(discoverySession, peer.peerHandle)
            .setPskPassphrase(peer.psk)
        if (isPublisher) {
            val localPort = getLocalDevice().port.toInt()
            builder.setPort(localPort)
            builder.setTransportProtocol(IPPROTO_UDP)
        }
        return builder.build()
    }

    private fun handleAwareNetworkReady(network: Network, info: WifiAwareNetworkInfo, peer: AwarePeer) {
        if (!connectedPeers.add(peer.deviceId)) return

        val peerAddress = info.peerIpv6Addr
        val peerIpv6 = peerAddress?.hostAddress?.substringBefore('%')
        val peerScopeId = peerAddress?.scopeId ?: 0
        val peerPort = info.port.takeIf { it > 0 } ?: peer.port
        if (peerIpv6 == null) {
            Log.w(TAG, "WiFi Aware network ready but missing peer IPv6 for ${peer.deviceId}")
            connectedPeers.remove(peer.deviceId)
            return
        }

        try {
            val localPort = getLocalDevice().port.toInt()
            val socket = DatagramSocket(null).apply {
                reuseAddress = true
                bind(InetSocketAddress(localPort))
            }
            network.bindSocket(socket)
            val boundPort = socket.localPort
            val pfd = ParcelFileDescriptor.fromDatagramSocket(socket)
            val fdNum = pfd.detachFd()
            injectAwareSocket(fdNum, peerIpv6, peerScopeId, peerPort.toUShort())
            connectedEndpoints[peer.deviceId] = Pair(peerIpv6, peerPort)
            Log.d(
                TAG,
                "Injected WiFi Aware socket for ${peer.deviceId}: localPort=$boundPort, peer=[$peerIpv6%$peerScopeId]:$peerPort",
            )
        } catch (e: Exception) {
            Log.w(TAG, "Failed to bind/inject WiFi Aware socket: ${e.message}", e)
            connectedPeers.remove(peer.deviceId)
            return
        }

        handler.post {
            onAwareConnected?.invoke(peer.deviceId, peerIpv6, peerPort)
        }
    }

    private fun unregisterAwareNetworks() {
        val cm = context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
        networkCallbacks.values.forEach { callback ->
            runCatching { cm.unregisterNetworkCallback(callback) }
        }
        networkCallbacks.clear()
        networkRequestStartedAt.clear()
        connectedPeers.clear()
        connectedEndpoints.clear()
    }

    private fun sendLocalInfo(peerHandle: PeerHandle) {
        val sub = subscribeSession ?: return
        val local = try {
            getLocalDevice()
        } catch (e: Exception) {
            Log.w(TAG, "Failed to get local device for WiFi Aware message", e)
            return
        }
        val info = buildServiceInfo(local.id, local.name, local.deviceType, local.port.toInt())
        runCatching { sub.sendMessage(peerHandle, 0, info) }
            .onFailure { Log.w(TAG, "Failed to send WiFi Aware peer info: ${it.message}") }
    }

    private fun generatePairPsk(localId: String, peerId: String): String {
        val ids = listOf(localId, peerId).sorted().joinToString(":")
        val mac = Mac.getInstance("HmacSHA256")
        mac.init(SecretKeySpec("connected-aware-pair-psk".toByteArray(), "HmacSHA256"))
        val hash = mac.doFinal(ids.toByteArray(StandardCharsets.UTF_8))
        return Base64.getEncoder().encodeToString(hash.copyOf(24))
    }

    private fun shouldInitiate(peerId: String): Boolean {
        val localId = try {
            getLocalDevice().id
        } catch (_: Exception) {
            return true
        }
        return localId > peerId
    }

    private val cleanupRunnable = object : Runnable {
        override fun run() {
            cleanupStaleState()
            handler.postDelayed(this, CLEANUP_INTERVAL_MS)
        }
    }

    private fun startCleanupLoop() {
        handler.removeCallbacks(cleanupRunnable)
        handler.postDelayed(cleanupRunnable, CLEANUP_INTERVAL_MS)
    }

    private fun cleanupStaleState() {
        val now = SystemClock.elapsedRealtime()
        val staleIds = mutableListOf<String>()

        for ((id, peer) in peers) {
            if (now - peer.lastSeenAtMs > STALE_PEER_MS) {
                staleIds.add(id)
            }
        }

        for (id in staleIds) {
            val peer = peers.remove(id)
            if (peer != null) {
                peersByHandle.remove(peer.peerHandle)
            }
            lastPeerLogAt.remove(id)
            networkRequestStartedAt.keys.removeIf { it.startsWith("$id:") }
            if (pendingPeerId == id) pendingPeerId = null
            if (pairingTargetId == id) pairingTargetId = null
        }
    }
}
