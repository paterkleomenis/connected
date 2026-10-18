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
        /**
         * Posted when the system stops the service for exceeding a foreground
         * service type's run-time limit. Must not collide with ConnectedApp's
         * request/progress/complete notification IDs (1001-1005).
         */
        private const val NOTIFICATION_ID_SERVICE_STOPPED = 1006

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

    /**
     * Android 15+ (API 35+) caps how long some foreground service types
     * (dataSync, mediaProcessing) may run — 6 hours per 24 hours. When the cap
     * is reached the system calls this method and stops the service within a
     * few seconds; if the service has not called stopSelf() by then, the
     * process is killed with a RemoteServiceException.
     *
     * The service only declares the connectedDevice type, which has no
     * run-time cap, so this is a safety net in case a future platform version
     * extends the caps to more types. It stops cleanly — so onDestroy() can
     * run its normal cleanup — and tells the user instead of dying silently.
     */
    override fun onTimeout(startId: Int, fgsType: Int) {
        Log.w(
            "ConnectedService",
            "Foreground service timed out (startId=$startId, fgsType=$fgsType) — stopping cleanly"
        )
        try {
            showServiceStoppedNotification()
        } catch (e: Exception) {
            Log.e("ConnectedService", "Failed to post service-stopped notification", e)
        }
        stopSelf()
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

        // onDestroy cannot postpone service destruction by returning. Always
        // release the service resources; durable transfers need a worker or a
        // separately managed foreground service instead.
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

        // Clear a "background sync stopped" notice from an earlier timeout, if
        // one is still showing.
        manager.cancel(NOTIFICATION_ID_SERVICE_STOPPED)

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
            startForegroundCompat(builder.build())
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
            startForegroundCompat(builder.build())
        } catch (_: Exception) { }
    }

    /**
     * Starts (or updates) the foreground notification. On API 29+ the
     * foreground service type is passed explicitly so the platform never
     * falls back to a broader type set with its own restrictions.
     */
    private fun startForegroundCompat(notification: Notification) {
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.Q) {
            startForeground(
                1,
                notification,
                android.content.pm.ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE
            )
        } else {
            startForeground(1, notification)
        }
    }

    /**
     * Posted from onTimeout() so a system-enforced stop is visible to the user
     * and can be undone with one tap.
     */
    private fun showServiceStoppedNotification() {
        val channelId = "connected_service_channel"
        val manager = getSystemService(NOTIFICATION_SERVICE) as NotificationManager

        val resumeIntent = Intent(this, MainActivity::class.java).apply {
            action = ConnectedApp.ACTION_RESUME_BACKGROUND_SERVICE
            flags = Intent.FLAG_ACTIVITY_NEW_TASK or
                Intent.FLAG_ACTIVITY_SINGLE_TOP or
                Intent.FLAG_ACTIVITY_CLEAR_TOP
        }
        val resumePendingIntent = android.app.PendingIntent.getActivity(
            this, 1, resumeIntent,
            android.app.PendingIntent.FLAG_UPDATE_CURRENT or android.app.PendingIntent.FLAG_IMMUTABLE
        )

        val notification = NotificationCompat.Builder(this, channelId)
            .setContentTitle("Background sync stopped")
            .setContentText("Android stopped the background service. Tap to resume.")
            .setSmallIcon(R.drawable.ic_notification_logo)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setContentIntent(resumePendingIntent)
            .setAutoCancel(true)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
            .setShowWhen(false)
            .build()

        manager.notify(NOTIFICATION_ID_SERVICE_STOPPED, notification)
    }
}
