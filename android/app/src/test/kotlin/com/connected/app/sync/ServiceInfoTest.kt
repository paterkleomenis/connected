package com.connected.app.sync

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Unit tests for the Wi-Fi Aware service-info codec.
 *
 * The NAN framework rejects publish/subscribe configs whose service-specific
 * info exceeds the driver's maximum length (255 bytes on most devices) with
 * an IllegalArgumentException, so the payload must be capped before it is
 * ever handed to the framework.
 *
 * These run on the JVM (no device/emulator) as part of
 * `:app:testSideloadDebugUnitTest`.
 */
class ServiceInfoTest {

    private val deviceId = "d3f2b7b0-2c6f-4a5b-9f1e-1234567890ab"

    @Test
    fun truncateUtf8_backsOffToCharacterBoundary() {
        assertEquals("", truncateUtf8("é", 1))
        assertEquals("é", truncateUtf8("é", 2))
        assertEquals("😀", truncateUtf8("😀😀", 6))
        assertEquals("", truncateUtf8("abc", 0))
        assertEquals("ab", truncateUtf8("abc", 2))
        assertEquals("abc", truncateUtf8("abc", 3))
    }

    @Test
    fun fitServiceInfoEntries_asciiFitsWithinDefaultLimit() {
        val entries = fitServiceInfoEntries(deviceId, "Pixel Test", "android", 47100, 255)
        assertTrue(encodeTxtRecords(entries).size <= 255)
        assertEquals("Pixel Test", entries["name"])
        assertEquals("Pixel Test", entries["pairingName"])
        assertEquals(deviceId, entries["id"])
        assertEquals("47100", entries["port"])
    }

    @Test
    fun fitServiceInfoEntries_longEmojiNameStillFits() {
        // 48 emoji = 192 UTF-8 bytes; the old char-based take(48) counted
        // characters, not bytes, and could overflow the payload limit.
        val name = "😀".repeat(48)
        val entries = fitServiceInfoEntries(deviceId, name, "android", 47100, 255)
        val encoded = encodeTxtRecords(entries)
        assertTrue("payload=${encoded.size}", encoded.size <= 255)
        val kept = entries.getValue("name")
        assertTrue(kept.isNotEmpty())
        assertTrue(name.startsWith(kept))
        assertEquals(kept, entries.getValue("pairingName"))
    }

    @Test
    fun fitServiceInfoEntries_smallDeviceLimitDropsOptionalFields() {
        val entries = fitServiceInfoEntries(deviceId, "Some Device", "android", 47100, 96)
        assertTrue(encodeTxtRecords(entries).size <= 96)
        assertFalse(entries.containsKey("vendorName"))
        assertFalse(entries.containsKey("modelName"))
        assertEquals(deviceId, entries.getValue("id"))
    }
}
