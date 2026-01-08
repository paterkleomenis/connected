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

        // Initialize the app logic with Application Context
        connectedApp = ConnectedApp(applicationContext)
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
                NotificationManager.IMPORTANCE_LOW
            )
            val manager = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
            manager.createNotificationChannel(channel)
        }

        val notification: Notification = androidx.core.app.NotificationCompat.Builder(this, channelId)
            .setContentTitle("Connected")
            .setContentText("Keeping connection alive...")
            .setSmallIcon(android.R.drawable.stat_notify_sync)
            .setPriority(androidx.core.app.NotificationCompat.PRIORITY_LOW)
            .build()

        // ID must be non-zero
        startForeground(1, notification)
    }
}
