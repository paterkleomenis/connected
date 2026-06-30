package com.connected.app.sync

import android.app.DownloadManager
import android.content.Context
import androidx.core.net.toUri
import android.os.Environment
import android.util.Log
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.io.BufferedReader
import java.io.InputStreamReader
import java.net.HttpURLConnection
import java.net.URL

data class UpdateInfo(
    val versionName: String,
    val downloadUrl: String
)

object AppUpdater {
    private const val TAG = "AppUpdater"
    private const val GITHUB_RELEASES_URL = "https://api.github.com/repos/paterkleomenis/connected/releases/latest"

    /**
     * Checks if the app was installed from the Play Store.
     */
    fun isPlayStoreInstall(context: Context): Boolean {
        try {
            if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.R) {
                val installSource = context.packageManager.getInstallSourceInfo(context.packageName)
                val installer = installSource.installingPackageName
                return installer != null && installer.startsWith("com.android.vending")
            } else {
                @Suppress("DEPRECATION")
                val installer = context.packageManager.getInstallerPackageName(context.packageName)
                return installer != null && installer.startsWith("com.android.vending")
            }
        } catch (e: Exception) {
            Log.e(TAG, "Error checking installer package", e)
        }
        return false
    }

    /**
     * Checks GitHub for the latest release.
     * Returns UpdateInfo if a newer version is found, null otherwise.
     */
    suspend fun checkForUpdate(currentVersion: String): UpdateInfo? = withContext(Dispatchers.IO) {
        try {
            val url = URL(GITHUB_RELEASES_URL)
            val connection = url.openConnection() as HttpURLConnection
            connection.requestMethod = "GET"
            connection.setRequestProperty("Accept", "application/vnd.github.v3+json")

            if (connection.responseCode == HttpURLConnection.HTTP_OK) {
                val reader = BufferedReader(InputStreamReader(connection.inputStream))
                val response = StringBuilder()
                var line: String?
                while (reader.readLine().also { line = it } != null) {
                    response.append(line)
                }
                reader.close()

                val json = JSONObject(response.toString())
                var tagName = json.getString("tag_name")
                if (tagName.startsWith("v", ignoreCase = true)) {
                    tagName = tagName.substring(1)
                }

                // Check version
                if (isNewerVersion(currentVersion, tagName)) {
                    val assets = json.getJSONArray("assets")
                    var downloadUrl: String? = null
                    for (i in 0 until assets.length()) {
                        val asset = assets.getJSONObject(i)
                        val name = asset.getString("name")
                        if (name.endsWith(".apk", ignoreCase = true)) {
                            downloadUrl = asset.getString("browser_download_url")
                            break
                        }
                    }

                    if (downloadUrl != null) {
                        return@withContext UpdateInfo(tagName, downloadUrl)
                    }
                }
            }
        } catch (e: Exception) {
            Log.e(TAG, "Failed to check for updates", e)
        }
        return@withContext null
    }

    /**
     * Downloads the APK and triggers install once completed via DownloadManager.
     */
    fun downloadUpdate(context: Context, downloadUrl: String, versionName: String) {
        try {
            val request = DownloadManager.Request(  downloadUrl.toUri())
                .setTitle("Downloading Connected $versionName")
                .setDescription("Downloading app update")
                .setNotificationVisibility(DownloadManager.Request.VISIBILITY_VISIBLE_NOTIFY_COMPLETED)
                .setDestinationInExternalPublicDir(Environment.DIRECTORY_DOWNLOADS, "connected-update-$versionName.apk")
                .setMimeType("application/vnd.android.package-archive")

            val downloadManager = context.getSystemService(Context.DOWNLOAD_SERVICE) as DownloadManager
            downloadManager.enqueue(request)
        } catch (e: Exception) {
            Log.e(TAG, "Failed to download update", e)
        }
    }

    /**
     * Simple version comparison: 3.2.0 vs 3.2.0
     * Returns true if `latestVersion` > `currentVersion`
     */
    private fun isNewerVersion(currentVersion: String, latestVersion: String): Boolean {
        val currentParts = currentVersion.split(".").mapNotNull { it.toIntOrNull() }
        val latestParts = latestVersion.split(".").mapNotNull { it.toIntOrNull() }

        val size = maxOf(currentParts.size, latestParts.size)
        for (i in 0 until size) {
            val current = currentParts.getOrElse(i) { 0 }
            val latest = latestParts.getOrElse(i) { 0 }
            if (latest > current) return true
            if (latest < current) return false
        }
        return false
    }
}
