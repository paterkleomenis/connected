package com.connected.app.sync

import org.junit.Assert.assertEquals
import org.junit.Test
import uniffi.connected_ffi.DeviceType
import java.util.Locale

/**
 * Unit tests for the small, framework-free helpers used by the UI.
 *
 * These run on the JVM (no device/emulator) as part of
 * `:app:testSideloadDebugUnitTest`.
 */
class FormattingTest {

    @Test
    fun formatFileSize_picksReadableUnit() {
        val previous = Locale.getDefault()
        Locale.setDefault(Locale.US)
        try {
            assertEquals("512 B", formatFileSize(512uL))
            assertEquals("1.0 KB", formatFileSize(1024uL))
            assertEquals("1.5 KB", formatFileSize(1536uL))
            assertEquals("1.0 MB", formatFileSize(1024uL * 1024uL))
            assertEquals("2.5 GB", formatFileSize((2.5 * 1024.0 * 1024.0 * 1024.0).toULong()))
        } finally {
            Locale.setDefault(previous)
        }
    }

    @Test
    fun displayName_coversEveryDeviceType() {
        assertEquals("Android", DeviceType.ANDROID.displayName())
        assertEquals("iOS", DeviceType.IOS.displayName())
        assertEquals("Linux", DeviceType.LINUX.displayName())
        assertEquals("Windows", DeviceType.WINDOWS.displayName())
        assertEquals("macOS", DeviceType.MAC_OS.displayName())
        assertEquals("Unknown", DeviceType.UNKNOWN.displayName())
    }
}
