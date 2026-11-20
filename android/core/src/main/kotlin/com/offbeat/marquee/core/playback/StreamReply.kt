package com.offbeat.marquee.core.playback

/**
 * Telling a song's bytes from a server's error answer.
 *
 * A Subsonic server answers an error on `stream` or `download` (a song it no longer has, a wrong
 * password) with HTTP 200 and a small JSON or XML body (the app asks for `f=json`), not with an
 * error status. Stored as it is, that body would be a "complete" song the player can never decode,
 * kept until the cache or the download is removed. The rule is the desktop client's: an answer whose
 * content type names JSON, XML or text is an error, never audio ([isNotAudio]); its Subsonic error
 * code says why ([subsonicCode], [kindOf]).
 *
 * What an older version already stored is recognised by [looksLikeErrorReply]: an entry too small
 * to be a song, or one that begins like JSON or XML (no audio format does).
 */
object StreamReply {
    /** Smaller than this, a stored song is an error answer: a second of the lowest Opus is bigger. */
    const val MIN_PLAUSIBLE_BYTES = 2048L

    /** Up to this size a stored song's first byte is looked at ([looksLikeErrorReply]). */
    const val CHECK_FIRST_BYTE_BELOW = 64L * 1024

    /** An answer of this content type is an error (JSON, XML, a text or HTML page), never audio. */
    fun isNotAudio(contentType: String?): Boolean {
        val ct = contentType?.lowercase() ?: return false
        return "json" in ct || "xml" in ct || ct.startsWith("text/")
    }

    private val jsonCode = Regex("\"error\"\\s*:\\s*\\{[^}]*\"code\"\\s*:\\s*(\\d+)")
    private val xmlCode = Regex("<error\\b[^>]*\\bcode\\s*=\\s*[\"'](\\d+)[\"']")

    /** The Subsonic error code in an error answer's body (JSON or XML), or null when it has none. */
    fun subsonicCode(body: String): Int? =
        (jsonCode.find(body) ?: xmlCode.find(body))?.groupValues?.get(1)?.toIntOrNull()

    /**
     * What a Subsonic error code means for playback: 40 to 44 (wrong user, password, token or API
     * key: the sign-in) is [PlaybackRecovery.FailureKind.Auth], 70 (not found) is NotFound, the rest
     * (and no code) Other.
     */
    fun kindOf(code: Int?): PlaybackRecovery.FailureKind = when (code) {
        in 40..44 -> PlaybackRecovery.FailureKind.Auth
        70 -> PlaybackRecovery.FailureKind.NotFound
        else -> PlaybackRecovery.FailureKind.Other
    }

    /**
     * A stored entry of [length] bytes, beginning with [firstByte] (null when it was not read, as
     * for entries of [CHECK_FIRST_BYTE_BELOW] or more), is an error answer and not a song.
     */
    fun looksLikeErrorReply(length: Long, firstByte: Int?): Boolean {
        if (length in 1 until MIN_PLAUSIBLE_BYTES) return true
        val b = firstByte ?: return false
        return b == '{'.code || b == '<'.code
    }

    /** Whether [looksLikeErrorReply] needs the first byte of an entry this long. */
    fun needsFirstByte(length: Long): Boolean = length in MIN_PLAUSIBLE_BYTES until CHECK_FIRST_BYTE_BELOW
}
