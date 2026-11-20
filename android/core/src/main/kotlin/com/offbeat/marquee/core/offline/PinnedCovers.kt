package com.offbeat.marquee.core.offline

import com.offbeat.marquee.core.art.CoverSize

/** One stored cover file: a cover id at a size bucket ([CoverSize]). */
data class CoverFile(val coverId: String, val size: Int)

/**
 * The covers a download keeps with it, outside the evictable cover cache, so a downloaded album or
 * playlist still has its art offline after the cover cache is cleared or evicted. A cover shared by
 * several pins is stored once and goes when the last pin that wants it goes (as songs do, [PinSet]).
 *
 * The pin's own cover is kept at every size it is drawn at (the grid, its page's header, Now
 * Playing, the thumbnails); the songs' covers (Navidrome gives each song its own id, and a playlist
 * mixes albums) at the thumbnail and header sizes only, which Now Playing scales up offline.
 */
object PinnedCovers {
    val MAIN_SIZES: Set<Int> = linkedSetOf(CoverSize.THUMB, CoverSize.GRID, CoverSize.HEADER, CoverSize.FULL)
    val SONG_SIZES: Set<Int> = linkedSetOf(CoverSize.THUMB, CoverSize.HEADER)

    /** At most this many song covers per pin (a long playlist), besides its own. */
    const val MAX_SONG_COVERS_PER_PIN = 200

    /** Every cover file the pins want, in pin order (the order they are fetched in). */
    fun wanted(pins: PinSet): LinkedHashSet<CoverFile> {
        val out = LinkedHashSet<CoverFile>()
        for (pin in pins.pins) {
            pin.coverArt?.let { c -> MAIN_SIZES.forEach { out += CoverFile(c, it) } }
            pin.songs.asSequence()
                .mapNotNull { it.coverArt }
                .filter { it != pin.coverArt }
                .distinct()
                .take(MAX_SONG_COVERS_PER_PIN)
                .forEach { c -> SONG_SIZES.forEach { out += CoverFile(c, it) } }
        }
        return out
    }

    /** What the cover folder must do to match the pins: fetch the missing, delete what nothing wants. */
    data class Plan(val fetch: List<CoverFile>, val delete: Set<CoverFile>)

    fun plan(wanted: Collection<CoverFile>, stored: Set<CoverFile>): Plan =
        Plan(wanted.filter { it !in stored }.distinct(), stored - wanted.toSet())

    /**
     * Which stored size answers a request for [requested]: the same size when it is stored, else the
     * smallest larger one (scaled down, no network needed). With nothing large enough it answers only
     * [offline] (the largest it has, scaled up, beats the placeholder); online the cache or the server
     * gives the sharper one. Null: not from the stored covers.
     */
    fun pick(requested: Int, stored: Set<Int>, offline: Boolean): Int? {
        if (stored.isEmpty()) return null
        if (requested in stored) return requested
        stored.filter { it > requested }.minOrNull()?.let { return it }
        return if (offline) stored.max() else null
    }

    // ---- File names ------------------------------------------------------------------------------

    private const val SEP = '@'
    private const val EXT = ".img"

    /** `<cover id, %-escaped outside [A-Za-z0-9._-]>@<size>.img`: safe on any file system, reversible. */
    fun fileName(f: CoverFile): String = buildString {
        for (ch in f.coverId) {
            if (ch.isLetterOrDigit() && ch.code < 128 || ch == '.' || ch == '_' || ch == '-') append(ch)
            else ch.toString().toByteArray(Charsets.UTF_8).forEach { append('%').append("%02X".format(it.toInt() and 0xFF)) }
        }
        append(SEP).append(f.size).append(EXT)
    }

    fun parse(name: String): CoverFile? {
        if (!name.endsWith(EXT)) return null
        val stem = name.removeSuffix(EXT)
        val at = stem.lastIndexOf(SEP)
        if (at <= 0) return null
        val size = stem.substring(at + 1).toIntOrNull()?.takeIf { it > 0 } ?: return null
        val id = unescape(stem.substring(0, at)) ?: return null
        return CoverFile(id, size)
    }

    private fun unescape(s: String): String? {
        val bytes = java.io.ByteArrayOutputStream()
        var i = 0
        while (i < s.length) {
            val ch = s[i]
            if (ch == '%') {
                if (i + 2 >= s.length) return null
                bytes.write(s.substring(i + 1, i + 3).toIntOrNull(16) ?: return null)
                i += 3
            } else {
                bytes.write(ch.code)
                i++
            }
        }
        return bytes.toString(Charsets.UTF_8.name())
    }
}
