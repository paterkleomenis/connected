package com.connected.app

import android.content.Context
import android.util.Log

object RustlsPlatformVerifier {
    init {
        try {
            System.loadLibrary("connected_ffi")
        } catch (e: UnsatisfiedLinkError) {
            Log.w("ConnectedApp", "Failed to load native library for TLS init", e)
        }
    }

    external fun init(context: Context)

    fun initIfNeeded(context: Context) {
        try {
            init(context.applicationContext)
        } catch (e: UnsatisfiedLinkError) {
            Log.w("ConnectedApp", "Native TLS init unavailable", e)
        } catch (e: Exception) {
            Log.w("ConnectedApp", "Native TLS init failed", e)
        }
    }
}
