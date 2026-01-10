package com.connected.app

import android.content.ComponentName
import android.content.Context
import android.media.session.MediaController
import android.media.session.MediaSessionManager
import android.media.session.PlaybackState
import android.service.notification.NotificationListenerService
import android.util.Log
import uniffi.connected_ffi.MediaState

class MediaObserverService : NotificationListenerService() {

    private var sessionManager: MediaSessionManager? = null
    private val sessionsChangedListener = object : MediaSessionManager.OnActiveSessionsChangedListener {
        override fun onActiveSessionsChanged(controllers: List<MediaController>?) {
            processControllers(controllers)
        }
    }

    private fun processControllers(controllers: List<MediaController>?) {
        if (controllers.isNullOrEmpty()) return

        // Prefer the first active controller (usually the one playing or last played)
        val controller = controllers.firstOrNull() ?: return

        // Register callback for this controller
        controller.registerCallback(object : MediaController.Callback() {
            override fun onPlaybackStateChanged(state: PlaybackState?) {
                broadcastState(controller)
            }

            override fun onMetadataChanged(metadata: android.media.MediaMetadata?) {
                broadcastState(controller)
            }
        })

        // Initial broadcast
        broadcastState(controller)
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
        sessionManager = getSystemService(Context.MEDIA_SESSION_SERVICE) as MediaSessionManager
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
        super.onDestroy()
        sessionManager?.removeOnActiveSessionsChangedListener(sessionsChangedListener)
    }
}
