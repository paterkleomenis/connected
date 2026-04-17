package com.connected.app.sync

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.AccessibilityServiceInfo
import android.content.ComponentName
import android.content.Context
import android.provider.Settings
import android.text.TextUtils
import android.util.Log
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import androidx.core.content.edit

class RcsAutoSendAccessibilityService : AccessibilityService() {

    companion object {
        private const val TAG = "RcsAutoSendA11y"

        private const val PREFS_NAME = "ConnectedPrefs"
        private const val PREF_AUTO_SEND_ENABLED = "telephony_auto_send_accessibility"
        private const val PREF_PENDING_REQUEST_TS = "telephony_auto_send_pending_timestamp"
        private const val PREF_PENDING_REQUEST_PACKAGE = "telephony_auto_send_pending_package"

        private const val REQUEST_TIMEOUT_MS = 20_000L

        fun queueAutoSend(context: Context, preferredPackage: String?) {
            context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE).edit {
                putLong(PREF_PENDING_REQUEST_TS, System.currentTimeMillis())
                putString(PREF_PENDING_REQUEST_PACKAGE, preferredPackage)
            }
        }

        fun clearPending(context: Context) {
            context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE).edit {
                remove(PREF_PENDING_REQUEST_TS)
                remove(PREF_PENDING_REQUEST_PACKAGE)
            }
        }

        fun isServiceEnabled(context: Context): Boolean {
            val enabled = try {
                Settings.Secure.getInt(
                    context.contentResolver,
                    Settings.Secure.ACCESSIBILITY_ENABLED
                ) == 1
            } catch (_: Exception) {
                false
            }

            if (!enabled) {
                return false
            }

            val componentName = ComponentName(context, RcsAutoSendAccessibilityService::class.java)
            val flattenedName = componentName.flattenToString()
            val flattenedShortName = componentName.flattenToShortString()

            val enabledServices = Settings.Secure.getString(
                context.contentResolver,
                Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES
            )

            if (TextUtils.isEmpty(enabledServices)) {
                return false
            }

            return enabledServices.split(':').any { component ->
                val trimmed = component.trim()
                trimmed == flattenedName ||
                        trimmed == flattenedShortName ||
                        trimmed.contains(RcsAutoSendAccessibilityService::class.java.name)
            }
        }
    }

    private data class PendingRequest(
        val timestampMs: Long,
        val preferredPackage: String?
    )

    override fun onServiceConnected() {
        super.onServiceConnected()
        serviceInfo = serviceInfo?.apply {
            eventTypes = AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED or
                    AccessibilityEvent.TYPE_WINDOW_CONTENT_CHANGED
            feedbackType = AccessibilityServiceInfo.FEEDBACK_GENERIC
            flags = flags or
                    AccessibilityServiceInfo.FLAG_REPORT_VIEW_IDS or
                    AccessibilityServiceInfo.FLAG_INCLUDE_NOT_IMPORTANT_VIEWS
            notificationTimeout = 80
        }
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
        if (event == null || !isAutoSendEnabled()) {
            return
        }

        val pending = getPendingRequest() ?: return
        val now = System.currentTimeMillis()
        if (now - pending.timestampMs > REQUEST_TIMEOUT_MS) {
            clearPending(this)
            return
        }

        val sourcePackage = event.packageName?.toString() ?: return
        if (!pending.preferredPackage.isNullOrBlank() && sourcePackage != pending.preferredPackage) {
            return
        }

        val root = rootInActiveWindow ?: return
        if (!hasMessageInputWithText(root)) {
            return
        }

        if (clickSendAction(root, sourcePackage)) {
            Log.d(TAG, "Triggered send click for package $sourcePackage")
            clearPending(this)
        }
    }

    override fun onInterrupt() {
        // No-op
    }

    private fun isAutoSendEnabled(): Boolean {
        return getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
            .getBoolean(PREF_AUTO_SEND_ENABLED, false)
    }

    private fun getPendingRequest(): PendingRequest? {
        val prefs = getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        val ts = prefs.getLong(PREF_PENDING_REQUEST_TS, 0L)
        if (ts <= 0L) {
            return null
        }

        return PendingRequest(
            timestampMs = ts,
            preferredPackage = prefs.getString(PREF_PENDING_REQUEST_PACKAGE, null)
        )
    }

    private fun hasMessageInputWithText(root: AccessibilityNodeInfo): Boolean {
        val queue = ArrayDeque<AccessibilityNodeInfo>()
        queue.add(root)

        while (queue.isNotEmpty()) {
            val node = queue.removeFirst()
            val className = node.className?.toString().orEmpty()
            val nodeText = node.text?.toString().orEmpty()

            if (className.contains("EditText") && nodeText.isNotBlank()) {
                return true
            }

            for (i in 0 until node.childCount) {
                node.getChild(i)?.let(queue::addLast)
            }
        }

        return false
    }

    private fun clickSendAction(root: AccessibilityNodeInfo, packageName: String): Boolean {
        val idCandidates = listOf(
            "send_message_button",
            "send_button",
            "compose_send",
            "send",
            "send_icon",
            "message_send_button"
        )

        for (id in idCandidates) {
            val fullId = "$packageName:id/$id"
            val nodes = root.findAccessibilityNodeInfosByViewId(fullId)
            for (node in nodes) {
                if (isLikelySendControl(node) && performClick(node)) {
                    return true
                }
            }
        }

        val queue = ArrayDeque<AccessibilityNodeInfo>()
        queue.add(root)

        while (queue.isNotEmpty()) {
            val node = queue.removeFirst()
            if (isLikelySendControl(node) && performClick(node)) {
                return true
            }

            for (i in 0 until node.childCount) {
                node.getChild(i)?.let(queue::addLast)
            }
        }

        return false
    }

    private fun isLikelySendControl(node: AccessibilityNodeInfo): Boolean {
        if (!node.isEnabled || !node.isVisibleToUser) {
            return false
        }

        val id = node.viewIdResourceName?.lowercase().orEmpty()
        val text = node.text?.toString()?.trim()?.lowercase().orEmpty()
        val desc = node.contentDescription?.toString()?.trim()?.lowercase().orEmpty()

        if (id.contains("resend") || text.contains("resend") || desc.contains("resend")) {
            return false
        }

        return id.contains("send") ||
                text == "send" ||
                desc.contains("send")
    }

    private fun performClick(node: AccessibilityNodeInfo): Boolean {
        if (node.isClickable && node.performAction(AccessibilityNodeInfo.ACTION_CLICK)) {
            return true
        }

        var parent = node.parent
        var depth = 0
        while (parent != null && depth < 6) {
            if (parent.isClickable && parent.performAction(AccessibilityNodeInfo.ACTION_CLICK)) {
                return true
            }
            parent = parent.parent
            depth++
        }

        return false
    }
}
