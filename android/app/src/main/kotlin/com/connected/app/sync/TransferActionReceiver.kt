package com.connected.app.sync

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log

class TransferActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val app = ConnectedApp.getInstance(context)

        fun transferRequestFromIntent(source: Intent): ConnectedApp.TransferRequest? {
            val transferId = source.getStringExtra(ConnectedApp.EXTRA_TRANSFER_ID) ?: return null
            val filename = source.getStringExtra(ConnectedApp.EXTRA_FILENAME).orEmpty()
            val fileSize = source.getLongExtra(ConnectedApp.EXTRA_FILE_SIZE, 0L).coerceAtLeast(0L).toULong()
            val fromDevice = source.getStringExtra(ConnectedApp.EXTRA_FROM_DEVICE).orEmpty()
            val fromFingerprint = source.getStringExtra(ConnectedApp.EXTRA_FINGERPRINT).orEmpty()
            return ConnectedApp.TransferRequest(transferId, filename, fileSize, fromDevice, fromFingerprint)
        }

        fun pairingRequestFromIntent(source: Intent): ConnectedApp.PairingRequest? {
            val deviceName = source.getStringExtra(ConnectedApp.EXTRA_DEVICE_NAME) ?: return null
            val fingerprint = source.getStringExtra(ConnectedApp.EXTRA_FINGERPRINT) ?: return null
            val deviceId = source.getStringExtra(ConnectedApp.EXTRA_DEVICE_ID) ?: return null
            return ConnectedApp.PairingRequest(deviceName, fingerprint, deviceId)
        }

        when (intent.action) {
            ConnectedApp.ACTION_ACCEPT_TRANSFER -> {
                val request = transferRequestFromIntent(intent) ?: return
                Log.d("TransferActionReceiver", "Accepting transfer ${request.id}")
                app.acceptTransfer(request)
            }

            ConnectedApp.ACTION_REJECT_TRANSFER -> {
                val request = transferRequestFromIntent(intent)
                if (request != null) {
                    Log.d("TransferActionReceiver", "Rejecting transfer ${request.id}")
                    app.rejectTransfer(request)
                } else {
                    Log.d("TransferActionReceiver", "Rejecting all pending transfers")
                    app.rejectAllTransfers()
                }
            }

            ConnectedApp.ACTION_ACCEPT_ALL_TRANSFERS -> {
                Log.d("TransferActionReceiver", "Accepting all pending transfers")
                app.acceptAllTransfers()
            }

            ConnectedApp.ACTION_CANCEL_TRANSFER -> {
                Log.d("TransferActionReceiver", "Cancelling active file transfer")
                app.cancelFileTransfer()
            }

            ConnectedApp.ACTION_ACCEPT_PAIRING -> {
                val request = pairingRequestFromIntent(intent) ?: return
                Log.d("TransferActionReceiver", "Trusting pairing for ${request.deviceId}")
                app.trustDevice(request)
            }

            ConnectedApp.ACTION_REJECT_PAIRING -> {
                val request = pairingRequestFromIntent(intent) ?: return
                Log.d("TransferActionReceiver", "Rejecting pairing for ${request.deviceId}")
                app.rejectDevice(request)
            }

            "com.connected.app.sync.ACTION_SHARE_CLIPBOARD" -> {
                Log.d("TransferActionReceiver", "Sharing clipboard")
                app.sendClipboardToAllTrusted()
            }
        }
    }
}
