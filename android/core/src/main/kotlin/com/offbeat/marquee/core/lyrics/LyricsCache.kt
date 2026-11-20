package com.offbeat.marquee.core.lyrics

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import java.io.File
import java.io.IOException

/**
 * What is known about one song's lyrics, from each source. A part is null/0 until that source was
 * asked; "nothing there" is remembered with the time it was asked, and asked again after
 * [LyricsCache.NOT_FOUND_TTL_MS] (lyrics added later are found then). Found lyrics are kept.
 */
@Serializable
data class LyricsEntry(
    /** The server's chosen lyrics; null with [serverAt] > 0 means it has none. */
    val server: Lyrics? = null,
    /** When the server was asked (wall clock ms); 0: never. */
    val serverAt: Long = 0,
    /** LRCLIB's answer: "found", "instrumental" or "not_found"; null: never asked. */
    val lrclibKind: String? = null,
    val lrclib: Lyrics? = null,
    val lrclibAt: Long = 0,
) {
    val lrclibAnswer: LyricsAnswer?
        get() = when (lrclibKind) {
            KIND_FOUND -> lrclib?.let { LyricsAnswer.Found(it) } ?: LyricsAnswer.NotFound
            KIND_INSTRUMENTAL -> LyricsAnswer.Instrumental
            KIND_NOT_FOUND -> LyricsAnswer.NotFound
            else -> null
        }

    fun withLrclib(a: LyricsAnswer, at: Long): LyricsEntry = when (a) {
        is LyricsAnswer.Found -> copy(lrclibKind = KIND_FOUND, lrclib = a.lyrics, lrclibAt = at)
        LyricsAnswer.Instrumental -> copy(lrclibKind = KIND_INSTRUMENTAL, lrclib = null, lrclibAt = at)
        LyricsAnswer.NotFound -> copy(lrclibKind = KIND_NOT_FOUND, lrclib = null, lrclibAt = at)
    }

    companion object {
        const val KIND_FOUND = "found"
        const val KIND_INSTRUMENTAL = "instrumental"
        const val KIND_NOT_FOUND = "not_found"
    }
}

/**
 * The lyrics answers on disk, one small JSON file per song id (`<dir>/<id>.json`; the key is the
 * song id only, never anything from the session). Least recently used out once the files pass
 * [maxBytes]. Writes are atomic (temp file, then rename). [clock] is the wall clock in ms (a fake
 * one in tests).
 */
class LyricsCache(
    private val dir: File,
    private val maxBytes: Long = DEFAULT_MAX_BYTES,
    private val clock: () -> Long = System::currentTimeMillis,
) {
    fun now(): Long = clock()

    @Synchronized
    fun read(songId: String): LyricsEntry? {
        val f = fileFor(songId)
        if (!f.isFile) return null
        return try {
            val e = json.decodeFromString(LyricsEntry.serializer(), f.readText())
            f.setLastModified(clock())
            e
        } catch (e: Exception) {
            // Unreadable (an older format, a torn write before rename existed): forget it.
            f.delete()
            null
        }
    }

    @Synchronized
    fun write(songId: String, entry: LyricsEntry) {
        try {
            dir.mkdirs()
            val f = fileFor(songId)
            val tmp = File(dir, f.name + ".tmp")
            tmp.writeText(json.encodeToString(LyricsEntry.serializer(), entry))
            if (!tmp.renameTo(f)) {
                f.delete()
                if (!tmp.renameTo(f)) tmp.delete()
            }
            f.setLastModified(clock())
            trim()
        } catch (e: IOException) {
            // A cache: losing a write only costs one more lookup.
        }
    }

    @Synchronized
    fun remove(songId: String) {
        fileFor(songId).delete()
    }

    @Synchronized
    fun clear() {
        dir.listFiles()?.forEach { it.delete() }
    }

    @Synchronized
    fun sizeBytes(): Long = entries().sumOf { it.length() }

    /** "Nothing there" is stale after the TTL, so it is asked again. */
    fun isStale(askedAt: Long): Boolean = askedAt <= 0 || clock() - askedAt >= NOT_FOUND_TTL_MS

    private fun entries(): List<File> = dir.listFiles { f -> f.isFile && f.name.endsWith(EXT) }?.toList().orEmpty()

    private fun trim() {
        val files = entries()
        var total = files.sumOf { it.length() }
        if (total <= maxBytes) return
        for (f in files.sortedBy { it.lastModified() }) {
            if (total <= maxBytes) break
            total -= f.length()
            f.delete()
        }
    }

    private fun fileFor(songId: String): File = File(dir, safeName(songId) + EXT)

    companion object {
        /** About 14 days, as on the desktop. */
        const val NOT_FOUND_TTL_MS = 14L * 24 * 3600 * 1000
        const val DEFAULT_MAX_BYTES = 4L * 1024 * 1024
        private const val EXT = ".json"

        private val json = Json { ignoreUnknownKeys = true; encodeDefaults = false }

        /** Ids are letters, digits, '-' and '_'; anything else becomes '_' (plus a hash, so two never collide). */
        fun safeName(id: String): String {
            val clean = id.map { if (it.isLetterOrDigit() && it.code < 128 || it == '-' || it == '_') it else '_' }.joinToString("").take(100)
            return if (clean == id) clean else clean + "~" + Integer.toHexString(id.hashCode())
        }
    }
}
