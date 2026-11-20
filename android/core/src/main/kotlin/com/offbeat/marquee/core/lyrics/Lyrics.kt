package com.offbeat.marquee.core.lyrics

import kotlinx.serialization.Serializable

/** Where lyrics came from: the user's own server (OpenSubsonic `getLyricsBySongId`) or LRCLIB. */
@Serializable
enum class LyricsSource { Server, Lrclib }

/** One line. [startMs] is when it is sung (ms from the start of the track), null for plain lyrics. */
@Serializable
data class LyricLine(val startMs: Long? = null, val text: String = "")

/**
 * Lyrics for a track. Synced lyrics have a start time on every line, in order; [offsetMs] is the
 * file's or the server's offset (positive shows the lines sooner, as in LRC and OpenSubsonic) and is
 * applied by [lineStart] and [currentLine], never baked into the lines.
 */
@Serializable
data class Lyrics(
    val synced: Boolean,
    val lines: List<LyricLine>,
    val source: LyricsSource,
    val offsetMs: Long = 0,
    val lang: String? = null,
) {
    val isEmpty: Boolean get() = lines.none { it.text.isNotBlank() }

    /** When line [i] starts on the track's clock (offset applied), or null for plain lyrics. */
    fun lineStart(i: Int): Long? = lines.getOrNull(i)?.startMs?.let { (it - offsetMs).coerceAtLeast(0) }

    /** The line being sung at [positionMs]: the last one that has started; -1 before the first. */
    fun currentLine(positionMs: Long): Int {
        if (!synced || lines.isEmpty()) return -1
        // Binary search for the last line with start <= position.
        var lo = 0
        var hi = lines.size
        while (lo < hi) {
            val mid = (lo + hi) ushr 1
            if ((lineStart(mid) ?: Long.MAX_VALUE) <= positionMs) lo = mid + 1 else hi = mid
        }
        return lo - 1
    }

    /**
     * When the highlighted line next changes after [positionMs] (the next line's start), or null
     * when the last line is showing. The view sleeps until then instead of checking every frame.
     */
    fun nextChangeAt(positionMs: Long): Long? {
        if (!synced) return null
        val next = currentLine(positionMs) + 1
        return lineStart(next)
    }
}

/** The answer for a track, as shown and as cached. */
sealed interface LyricsAnswer {
    data class Found(val lyrics: Lyrics) : LyricsAnswer

    /** LRCLIB says the track has no words. */
    data object Instrumental : LyricsAnswer

    data object NotFound : LyricsAnswer
}
