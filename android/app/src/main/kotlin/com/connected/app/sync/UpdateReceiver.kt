package com.connected.app.sync

import android.app.DownloadManager
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import android.util.Log

class UpdateReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        // Direct APK installs are sideload-flavor only. The playStore flavor
        // strips REQUEST_INSTALL_PACKAGES, so ignore downloads there.
        if (BuildConfig.FLAVOR != "sideload") return
        if (intent.action == DownloadManager.ACTION_DOWNLOAD_COMPLETE) {
            val downloadId = intent.getLongExtra(DownloadManager.EXTRA_DOWNLOAD_ID, -1)
            if (downloadId != -1L) {
                installApk(context, downloadId)
            }
        }
    }

    private fun installApk(context: Context, downloadId: Long) {
        try {
            // Since API 26, sideloading requires REQUEST_INSTALL_PACKAGES to be
            // declared AND the user must have granted "install unknown apps" for
            // this app. Without the grant the installer activity is silently
            // refused — route the user to the settings page instead of failing.
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O &&
                !context.packageManager.canRequestPackageInstalls()
            ) {
                Log.w(
                    "UpdateReceiver",
                    "Install-unknown-apps permission not granted; opening settings"
                )
                try {
                    val settingsIntent = Intent(
                        Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
                        Uri.parse("package:${context.packageName}")
                    ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                    context.startActivity(settingsIntent)
                } catch (e: Exception) {
                    Log.e("UpdateReceiver", "Failed to open unknown-app settings", e)
                }
                return
            }

            val downloadManager = context.getSystemService(Context.DOWNLOAD_SERVICE) as DownloadManager
            downloadManager.query(DownloadManager.Query().setFilterById(downloadId)).use { cursor ->
                if (!cursor.moveToFirst()) return
                val status = cursor.getInt(
                    cursor.getColumnIndexOrThrow(DownloadManager.COLUMN_STATUS)
                )
                if (status != DownloadManager.STATUS_SUCCESSFUL) return
            }
            val uri = downloadManager.getUriForDownloadedFile(downloadId) ?: return

            val mimeType = downloadManager.getMimeTypeForDownloadedFile(downloadId)
            if (mimeType != "application/vnd.android.package-archive") return

            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O &&
                !context.packageManager.canRequestPackageInstalls()
            ) {
                val settingsIntent = Intent(
                    Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
                    Uri.parse("package:${context.packageName}")
                ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                context.startActivity(settingsIntent)
                return
            }

            val installIntent = Intent(Intent.ACTION_VIEW).apply {
                setDataAndType(uri, mimeType)
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            }
            context.startActivity(installIntent)
        } catch (e: Exception) {
            Log.e("UpdateReceiver", "Failed to start install intent", e)
        }
    }
}
