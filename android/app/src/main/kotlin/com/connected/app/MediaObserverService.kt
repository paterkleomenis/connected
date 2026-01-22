package com.connected.app

import android.app.Notification
import android.content.ComponentName
import android.media.session.MediaController
import android.media.session.MediaSessionManager
import android.media.session.PlaybackState
import android.service.notification.NotificationListenerService
import android.service.notification.StatusBarNotification
import android.util.Log
import androidx.core.os.BundleCompat
import uniffi.connected_ffi.MediaState

class MediaObserverService : NotificationListenerService() {

    private var sessionManager: MediaSessionManager? = null
    private val sessionsChangedListener =
        MediaSessionManager.OnActiveSessionsChangedListener { controllers -> processControllers(controllers) }

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
        super.onDestroy()
        sessionManager?.removeOnActiveSessionsChangedListener(sessionsChangedListener)
    }

    override fun onNotificationPosted(sbn: StatusBarNotification) {
        if (sbn.packageName != "com.google.android.apps.messaging") {
            return
        }

        val notification = sbn.notification
        val extras = notification.extras

        Log.d("MediaObserver", "Google Messages notification posted: key=${sbn.key}, group=${sbn.groupKey}")

        val messagingBody = extractMessagingStyleBody(extras)
        val body = messagingBody?.first
            ?: extras.getCharSequence(Notification.EXTRA_TEXT)
            ?: extractTextLines(extras)
            ?: extras.getCharSequence(Notification.EXTRA_BIG_TEXT)
            ?: return
        val title = extras.getCharSequence(Notification.EXTRA_CONVERSATION_TITLE)
            ?: extras.getCharSequence(Notification.EXTRA_TITLE)
            ?: messagingBody?.second
            ?: "Unknown"

        val key = sbn.key
        val bodyString = body.toString()
        val titleString = title.toString()
        if (!shouldRelay(key, bodyString)) {
            return
        }

        ConnectedApp.getInstance()?.relayRcsNotification(
            titleString,
            bodyString,
            sbn.postTime
        )
    }

    companion object {
        private val recentByKey = LinkedHashMap<String, String>()

        private fun shouldRelay(key: String, body: String): Boolean {
            val lastBody = recentByKey[key]
            if (lastBody == body) {
                return false
            }
            recentByKey[key] = body
            if (recentByKey.size > 50) {
                val firstKey = recentByKey.keys.firstOrNull()
                if (firstKey != null) {
                    recentByKey.remove(firstKey)
                }
            }
            return true
        }

        private fun extractMessagingStyleBody(extras: android.os.Bundle): Pair<CharSequence, CharSequence?>? {
            val messages = BundleCompat.getParcelableArray(
                extras,
                Notification.EXTRA_MESSAGES,
                android.os.Parcelable::class.java
            ) ?: return null
            if (messages.isEmpty()) return null
            val last = messages.lastOrNull() as? android.os.Bundle ?: return null
            val text = (last.getCharSequence("text") ?: last.getCharSequence("android.text")) ?: return null
            val sender = last.getCharSequence("sender") ?: last.getCharSequence("android.sender")
            return text to sender
        }

        private fun extractTextLines(extras: android.os.Bundle): CharSequence? {
            val lines = extras.getCharSequenceArray(Notification.EXTRA_TEXT_LINES) ?: return null
            if (lines.isEmpty()) return null
            return lines.last()
        }
    }
}
