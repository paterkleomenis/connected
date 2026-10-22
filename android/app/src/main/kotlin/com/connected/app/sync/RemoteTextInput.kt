package com.connected.app.sync

import java.text.BreakIterator
import java.util.Locale

/** Maximum amount of live-typed text kept on the phone. */
internal const val MAX_LIVE_TEXT_LENGTH = 512

/**
 * A live edit: erase [backspaces] user-perceived characters at the remote
 * caret, then insert [insert].
 */
internal data class RemoteTextEdit(
    val backspaces: Int,
    val insert: String
)

/**
 * Plans the remote edit that turns [previous] into [next].
 *
 * The desktop is not mirrored in full: the user can move the desktop caret
 * independently, or type directly there. Therefore the common prefix is kept
 * and the rest of the suffix is rewritten. That covers normal typing,
 * backspace, selection replacement, and corrections in the middle of the
 * phone field while keeping the remote text consistent with the phone field.
 */
internal fun planRemoteTextEdit(previous: String, next: String): RemoteTextEdit? {
    if (previous == next) return null

    var common = 0
    val maxCommon = minOf(previous.length, next.length)
    while (common < maxCommon && previous[common] == next[common]) {
        common++
    }

    // Never split a surrogate pair. A replacement emoji often shares its high
    // surrogate with the previous one, so a naive common prefix can otherwise
    // address half a code point.
    if (common in 1 until previous.length &&
        Character.isHighSurrogate(previous[common - 1]) &&
        Character.isLowSurrogate(previous[common])
    ) {
        common--
    }

    val removed = previous.substring(common)
    val inserted = next.substring(common)
    if (removed.isEmpty() && inserted.isEmpty()) return null
    return RemoteTextEdit(
        backspaces = remoteBackspaceCount(removed),
        insert = inserted
    )
}

/** Truncates live text without splitting a surrogate pair. */
internal fun limitLiveText(value: String): String {
    if (value.length <= MAX_LIVE_TEXT_LENGTH) return value
    var end = MAX_LIVE_TEXT_LENGTH
    if (Character.isHighSurrogate(value[end - 1])) end--
    return value.substring(0, end)
}

/** Number of Backspace presses needed to erase [text] at the remote caret. */
internal fun remoteBackspaceCount(text: String): Int {
    if (text.isEmpty()) return 0
    val iterator = BreakIterator.getCharacterInstance(Locale.ROOT)
    iterator.setText(text)
    iterator.first()
    var count = 0
    while (iterator.next() != BreakIterator.DONE) {
        count++
    }
    return count
}

/** Removes one user-perceived character from the end of [text]. */
internal fun dropLastGrapheme(text: String): String {
    if (text.isEmpty()) return text
    val iterator = BreakIterator.getCharacterInstance(Locale.ROOT)
    iterator.setText(text)
    iterator.last()
    val start = iterator.previous()
    return if (start == BreakIterator.DONE) "" else text.substring(0, start)
}
