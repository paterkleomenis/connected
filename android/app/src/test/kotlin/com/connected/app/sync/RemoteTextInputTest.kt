package com.connected.app.sync

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class RemoteTextInputTest {

    @Test
    fun appendSendsOnlyTheNewSuffix() {
        assertEquals(
            RemoteTextEdit(backspaces = 0, insert = " world"),
            planRemoteTextEdit("hello", "hello world")
        )
    }

    @Test
    fun backspaceCountsDeletedCharacters() {
        assertEquals(
            RemoteTextEdit(backspaces = 3, insert = ""),
            planRemoteTextEdit("hello", "he")
        )
    }

    @Test
    fun replacementRewritesTheRemainingSuffix() {
        assertEquals(
            RemoteTextEdit(backspaces = 3, insert = "Xlo"),
            planRemoteTextEdit("hello", "heXlo")
        )
    }

    @Test
    fun middleInsertionRewritesTheRemainingSuffix() {
        assertEquals(
            RemoteTextEdit(backspaces = 3, insert = "Xllo"),
            planRemoteTextEdit("hello", "heXllo")
        )
    }

    @Test
    fun unchangedTextProducesNoEdit() {
        assertNull(planRemoteTextEdit("hello", "hello"))
    }

    @Test
    fun emojiIsOneBackspace() {
        assertEquals(
            RemoteTextEdit(backspaces = 1, insert = ""),
            planRemoteTextEdit("a😀", "a")
        )
    }

    @Test
    fun replacingEmojiDoesNotSplitSurrogatePairs() {
        assertEquals(
            RemoteTextEdit(backspaces = 1, insert = "😁"),
            planRemoteTextEdit("a😀", "a😁")
        )
    }

    @Test
    fun limitDoesNotSplitSurrogatePairs() {
        val value = "a".repeat(MAX_LIVE_TEXT_LENGTH - 1) + "😀"
        val limited = limitLiveText(value)
        assertEquals(MAX_LIVE_TEXT_LENGTH - 1, limited.length)
        assertEquals("a".repeat(MAX_LIVE_TEXT_LENGTH - 1), limited)
    }

    @Test
    fun dropLastGraphemeKeepsSurrogatePairsWhole() {
        assertEquals("a", dropLastGrapheme("a😀"))
    }
}
