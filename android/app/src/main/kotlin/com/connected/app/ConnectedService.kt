package com.connected.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.os.Binder
import android.os.IBinder
import android.util.Log

class ConnectedService : Service() {
    private val binder = LocalBinder()
    lateinit var connectedApp: ConnectedApp
        private set

    inner class LocalBinder : Binder() {
        fun getService(): ConnectedService = this@ConnectedService
    }

    override fun onCreate() {
        super.onCreate()
        Log.d("ConnectedService", "Creating service")

        // Initialize the app logic with Application Context via Singleton
        connectedApp = ConnectedApp.getInstance(applicationContext)
        // We generally expect the app to be initialized, but if the service starts fresh (e.g. boot),
        // we must ensure it's initialized. initialize() should be idempotent-ish.
        connectedApp.initialize()

        startForegroundService()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        Log.d("ConnectedService", "Service started")
        // If the system kills the service, recreate it
        return START_STICKY
    }

    override fun onBind(intent: Intent): IBinder {
        return binder
    }

    override fun onDestroy() {
        Log.d("ConnectedService", "Destroying service")
        connectedApp.cleanup()
        super.onDestroy()
    }

    private fun startForegroundService() {
        val channelId = "connected_service_channel"
        val channelName = "Connected Background Service"

        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                channelId,
                channelName,
                NotificationManager.IMPORTANCE_MAX
            ).apply {
                lockscreenVisibility = Notification.VISIBILITY_PUBLIC
                setShowBadge(false)
            }
            val manager = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
            manager.createNotificationChannel(channel)
        }

        val shareIntent = Intent(this, ClipboardHelperActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK
        }
        val sharePendingIntent = android.app.PendingIntent.getActivity(
            this,
            0,
            shareIntent,
            android.app.PendingIntent.FLAG_UPDATE_CURRENT or android.app.PendingIntent.FLAG_IMMUTABLE
        )

        val builder = androidx.core.app.NotificationCompat.Builder(this, channelId)
            .setContentTitle("Connected")
            .setContentText("Click to share clipboard")
            .setSmallIcon(android.R.drawable.stat_notify_sync)
            .setPriority(androidx.core.app.NotificationCompat.PRIORITY_MAX)
            .setCategory(androidx.core.app.NotificationCompat.CATEGORY_SERVICE)
            .setVisibility(androidx.core.app.NotificationCompat.VISIBILITY_PUBLIC)
            .setShowWhen(false)
            .setContentIntent(sharePendingIntent)
            .setColorized(true)
            .setColor(0xFF6200EE.toInt()) // purple_500
            .setOngoing(true)

        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.S) {
            builder.setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE)
        }

        val notification = builder.build()

        // ID must be non-zero
        startForeground(1, notification)
    }
}
