package com.connected.app

import android.app.Activity
import android.content.ClipboardManager
import android.os.Build
import android.os.Bundle
import android.widget.Toast
import android.os.Handler
import android.os.Looper

class ClipboardHelperActivity : Activity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // No setContentView needed for invisible activity
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus) {
            // Slight delay to ensure system recognizes focus
            Handler(Looper.getMainLooper()).postDelayed({
                shareClipboard()
            }, 100)
        }
    }

    private fun shareClipboard() {
        try {
            val clipboard = getSystemService(CLIPBOARD_SERVICE) as ClipboardManager

            // Check if clipboard has data
            if (!clipboard.hasPrimaryClip()) {
                Toast.makeText(this, "Clipboard is empty", Toast.LENGTH_SHORT).show()
                finish()
                return
            }

            val clipData = clipboard.primaryClip
            if (clipData != null && clipData.itemCount > 0) {
                // Try to get text, then coerce to text if needed
                val item = clipData.getItemAt(0)
                val text = item.text?.toString() ?: item.coerceToText(this)?.toString()

                if (!text.isNullOrEmpty()) {
                    val app = ConnectedApp.getInstance(applicationContext)
                    app.broadcastClipboard(text)
                } else {
                    Toast.makeText(this, "Clipboard content not text", Toast.LENGTH_SHORT).show()
                }
            } else {
                Toast.makeText(this, "Clipboard is empty", Toast.LENGTH_SHORT).show()
            }
        } catch (e: Exception) {
            Toast.makeText(this, "Error accessing clipboard", Toast.LENGTH_SHORT).show()
            e.printStackTrace()
        } finally {
            finish()
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                overrideActivityTransition(OVERRIDE_TRANSITION_CLOSE, 0, 0)
            } else {
                @Suppress("DEPRECATION")
                overridePendingTransition(0, 0)
            }
        }
    }
}
