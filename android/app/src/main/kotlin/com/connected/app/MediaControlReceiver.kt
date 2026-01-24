package com.connected.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log
import uniffi.connected_ffi.MediaCommand

class MediaControlReceiver : BroadcastReceiver() {
    companion object {
        const val ACTION_PLAY_PAUSE = "com.connected.app.ACTION_MEDIA_PLAY_PAUSE"
        const val ACTION_NEXT = "com.connected.app.ACTION_MEDIA_NEXT"
        const val ACTION_PREVIOUS = "com.connected.app.ACTION_MEDIA_PREVIOUS"
    }

    override fun onReceive(context: Context, intent: Intent) {
        val app = ConnectedApp.getInstance(context)

        val command = when (intent.action) {
            ACTION_PLAY_PAUSE -> {
                Log.d("MediaControlReceiver", "Play/Pause command received")
                MediaCommand.PLAY_PAUSE
            }
            ACTION_NEXT -> {
                Log.d("MediaControlReceiver", "Next command received")
                MediaCommand.NEXT
            }
            ACTION_PREVIOUS -> {
                Log.d("MediaControlReceiver", "Previous command received")
                MediaCommand.PREVIOUS
            }
            else -> return
        }

        app.sendMediaCommandToLastDevice(command)
    }
}
