package com.connected.app.sync

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Intent
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationCompat

class ConnectedService : Service() {
    companion object {
        @Volatile
        var isRunning = false
            private set

        @Volatile
        private var activeInstance: ConnectedService? = null

        fun updateForegroundNotification(title: String, content: String, percent: Int) {
            activeInstance?.updateNotification(title, content, percent)
        }

        fun restoreDefaultForegroundNotification() {
            activeInstance?.restoreDefaultNotification()
        }
    }

    lateinit var connectedApp: ConnectedApp
        private set

    override fun onCreate() {
        super.onCreate()
        Log.d("ConnectedService", "Creating service")
        isRunning = true
        activeInstance = this

        connectedApp = ConnectedApp.getInstance(applicationContext)
        connectedApp.initialize()

        startForegroundService()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        Log.d("ConnectedService", "Service started")
        return START_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? {
        return null
    }

    override fun onDestroy() {
        Log.d("ConnectedService", "Destroying service")
        isRunning = false
        activeInstance = null

        // Always cancel active transfers first so the peer receives a
        // Cancel message even if the process gets killed afterwards.
        if (connectedApp.hasActiveTransfers()) {
            Log.d("ConnectedService", "Active transfers in progress — cancelling transfers")
            connectedApp.cancelFileTransfer()
        }

        // If there are still active transfers after cancellation, keep the
        // process alive so they can complete in the background.
        if (connectedApp.hasActiveTransfers()) {
            Log.d("ConnectedService", "Transfers still active — keeping foreground alive")
            updateNotification("Connected", "Transfer in progress...", -1)
            return
        }

        // No active transfers — normal cleanup
        stopForeground(STOP_FOREGROUND_REMOVE)
        Thread {
            try {
                connectedApp.cleanup()
                Log.d("ConnectedService", "Cleanup completed successfully")
            } catch (e: Exception) {
                Log.e("ConnectedService", "Error during cleanup: ${e.message}", e)
            }
        }.start()
        super.onDestroy()
    }

    private fun startForegroundService() {
        val channelId = "connected_service_channel"
        val channelName = "Connected Background Service"

        val channel = NotificationChannel(
            channelId,
            channelName,
            NotificationManager.IMPORTANCE_HIGH
        ).apply {
            lockscreenVisibility = Notification.VISIBILITY_PUBLIC
            setShowBadge(false)
        }
        val manager = getSystemService(NOTIFICATION_SERVICE) as NotificationManager
        manager.createNotificationChannel(channel)

        restoreDefaultNotification()
    }

    private fun updateNotification(title: String, content: String, percent: Int) {
        val channelId = "connected_service_channel"
        val builder = NotificationCompat.Builder(this, channelId)
            .setContentTitle(title)
            .setContentText(content)
            .setSmallIcon(R.drawable.ic_notification_logo)
            .setPriority(NotificationCompat.PRIORITY_MAX)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
            .setShowWhen(false)
            .setOngoing(true)
            .setColorized(true)
            .setColor(0xFFFFFFFF.toInt())

        if (percent >= 0) {
            builder.setProgress(100, percent, false)
        }

        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.S) {
            builder.setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE)
        }

        try {
            startForeground(1, builder.build())
        } catch (_: Exception) { }
    }

    private fun restoreDefaultNotification() {
        val channelId = "connected_service_channel"
        val shareIntent = Intent(this, ClipboardHelperActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK
        }
        val sharePendingIntent = android.app.PendingIntent.getActivity(
            this, 0, shareIntent,
            android.app.PendingIntent.FLAG_UPDATE_CURRENT or android.app.PendingIntent.FLAG_IMMUTABLE
        )

        val builder = NotificationCompat.Builder(this, channelId)
            .setContentTitle("Connected")
            .setContentText("Click to share clipboard")
            .setSmallIcon(R.drawable.ic_notification_logo)
            .setPriority(NotificationCompat.PRIORITY_MAX)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
            .setShowWhen(false)
            .setContentIntent(sharePendingIntent)
            .setOngoing(true)
            .setColorized(true)
            .setColor(0xFFFFFFFF.toInt())

        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.S) {
            builder.setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE)
        }

        try {
            startForeground(1, builder.build())
        } catch (_: Exception) { }
    }
}
