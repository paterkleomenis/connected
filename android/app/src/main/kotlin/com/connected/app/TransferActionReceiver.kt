package com.connected.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log

class TransferActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val transferId = intent.getStringExtra("transferId")
        val app = ConnectedApp.getInstance(context)

        when (intent.action) {
            "com.connected.app.ACTION_ACCEPT_TRANSFER" -> {
                if (transferId == null) return
                Log.d("TransferActionReceiver", "Accepting transfer $transferId")
                // We need to reconstruct the request object or find it
                // Ideally ConnectedApp stores the pending request.
                // For now, we'll create a dummy object with the ID to pass to acceptTransfer
                val request = ConnectedApp.TransferRequest(transferId, "", 0u, "")
                app.acceptTransfer(request)
            }

            "com.connected.app.ACTION_REJECT_TRANSFER" -> {
                if (transferId == null) return
                Log.d("TransferActionReceiver", "Rejecting transfer $transferId")
                val request = ConnectedApp.TransferRequest(transferId, "", 0u, "")
                app.rejectTransfer(request)
            }

            "com.connected.app.ACTION_SHARE_CLIPBOARD" -> {
                Log.d("TransferActionReceiver", "Sharing clipboard")
                app.sendClipboardToAllTrusted()
            }
        }
    }
}
