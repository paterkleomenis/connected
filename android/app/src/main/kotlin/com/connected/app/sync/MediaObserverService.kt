package com.connected.app.sync

import android.content.ComponentName
import android.media.session.MediaController
import android.media.session.MediaSessionManager
import android.media.session.PlaybackState
import android.service.notification.NotificationListenerService
import android.util.Log
import uniffi.connected_ffi.MediaState

class MediaObserverService : NotificationListenerService() {

    private var sessionManager: MediaSessionManager? = null
    private val sessionsChangedListener =
        MediaSessionManager.OnActiveSessionsChangedListener { controllers -> processControllers(controllers) }

    // Track the currently-registered callback + its controller so we can
    // unregister before registering a new one. Registering a fresh anonymous
    // callback on every sessions-changed event leaked callbacks (each holding
    // the controller and delivering stale events) for the service's lifetime.
    private var activeController: MediaController? = null
    private var activeControllerCallback: MediaController.Callback? = null

    private fun processControllers(controllers: List<MediaController>?) {
        // Prefer the first active controller (usually the one playing or last
        // played), ignoring our own session: it mirrors the remote device's
        // playback, so reporting it back would echo state to the peer.
        val controller = controllers.orEmpty().firstOrNull { it.packageName != packageName }

        // Re-registering on a different controller? Drop the old callback.
        unregisterActiveCallback()

        if (controller == null) return

        val callback = object : MediaController.Callback() {
            override fun onPlaybackStateChanged(state: PlaybackState?) {
                broadcastState(controller)
            }

            override fun onMetadataChanged(metadata: android.media.MediaMetadata?) {
                broadcastState(controller)
            }
        }
        controller.registerCallback(callback)
        activeController = controller
        activeControllerCallback = callback

        // Initial broadcast
        broadcastState(controller)
    }

    private fun unregisterActiveCallback() {
        val cb = activeControllerCallback ?: return
        try {
            activeController?.unregisterCallback(cb)
        } catch (_: Exception) {
            // Controller already died — nothing to unregister
        }
        activeController = null
        activeControllerCallback = null
    }

    private fun broadcastState(controller: MediaController) {
        val state = controller.playbackState
        val meta = controller.metadata

        val isPlaying = state?.state == PlaybackState.STATE_PLAYING

        val title = meta?.getString(android.media.MediaMetadata.METADATA_KEY_TITLE)
        val artist = meta?.getString(android.media.MediaMetadata.METADATA_KEY_ARTIST)
        val album = meta?.getString(android.media.MediaMetadata.METADATA_KEY_ALBUM)

        Log.d("MediaObserver", "Broadcasting state: $title by $artist (Playing: $isPlaying)")

        // Send to ConnectedApp logic
        ConnectedApp.getInstance()?.onLocalMediaUpdate(MediaState(title, artist, album, isPlaying))
    }

    override fun onCreate() {
        super.onCreate()
        Log.d("MediaObserver", "MediaObserverService Created")
        sessionManager = getSystemService(MEDIA_SESSION_SERVICE) as MediaSessionManager
        try {
            val componentName = ComponentName(this, MediaObserverService::class.java)
            sessionManager?.addOnActiveSessionsChangedListener(sessionsChangedListener, componentName)
            // Initial check
            val controllers = sessionManager?.getActiveSessions(componentName)
            processControllers(controllers)
        } catch (e: SecurityException) {
            Log.e("MediaObserver", "Permission missing for MediaSessionManager", e)
        }
    }

    override fun onDestroy() {
        unregisterActiveCallback()
        super.onDestroy()
        sessionManager?.removeOnActiveSessionsChangedListener(sessionsChangedListener)
    }

}
