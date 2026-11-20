package com.offbeat.marquee.core.playback

import com.offbeat.marquee.core.subsonic.Song
import com.offbeat.marquee.core.subsonic.SubsonicClient

/**
 * What the server is asked to send for one track. [format] null never happens on the wire: the
 * original is asked for with `format=raw`, so a server-side transcoding rule for this client can't
 * silently replace it.
 */
data class StreamQuality(val id: String, val format: String, val maxBitRate: Int?) {
    val isOriginal: Boolean get() = format == "raw"

    /**
     * For an Opus quality, the highest bitrate an Opus file may have and still be sent as it is
     * ([forSource]); null for the original.
     */
    val opusUnchangedUpTo: Int? get() = if (format == "opus") maxBitRate?.let { it * OPUS_UNCHANGED_RATIO } else null

    /**
     * The quality to ask for, for this file: an Opus quality becomes the [ORIGINAL] when the file is
     * already Opus at no more than [opusUnchangedUpTo]. Unknown files and bitrates keep the quality.
     */
    fun forSource(source: SourceFile?): StreamQuality {
        val limit = opusUnchangedUpTo ?: return this
        if (source == null || !source.isOpus) return this
        val kbps = source.bitRate?.takeIf { it > 0 } ?: return this
        return if (kbps <= limit) ORIGINAL else this
    }

    companion object {
        val ORIGINAL = StreamQuality("original", "raw", null)

        fun opus(kbps: Int): StreamQuality = StreamQuality("opus$kbps", "opus", kbps)

        /** Every quality a copy may be stored in (Settings offers [com.offbeat.marquee.core.settings.QualityChoices.OFFERED]). */
        val CHOICES: List<StreamQuality> = listOf(ORIGINAL, opus(320), opus(192), opus(128), opus(96), opus(64))

        /**
         * Asking the server for Opus from a file that is already Opus makes it re-encode live, and a
         * live transcode has no length and no ranges, so the track can't be seeked; it also saves
         * little (about 30 % from 180 to 128 kbps) for a loss in quality. Up to twice the wanted
         * bitrate the original is sent instead; past that (Opus 64 chosen to save data) the
         * saving is worth it and the transcode stays.
         */
        const val OPUS_UNCHANGED_RATIO = 2

        val DEFAULT_WIFI = ORIGINAL
        val DEFAULT_MOBILE = opus(128)

        /** The stored [id] back to a quality; unknown ids fall back to [fallback]. */
        fun fromId(id: String?, fallback: StreamQuality): StreamQuality {
            if (id == null) return fallback
            CHOICES.firstOrNull { it.id == id }?.let { return it }
            val kbps = id.removePrefix("opus").toIntOrNull()
            return if (id.startsWith("opus") && kbps != null && kbps in 16..512) opus(kbps) else fallback
        }
    }
}

/**
 * The song's file on the server, as far as the quality rules need it (the song's `suffix`,
 * `contentType` and `bitRate`, in kbps; 0 or null when the server doesn't know).
 */
data class SourceFile(val suffix: String?, val contentType: String?, val bitRate: Int?) {
    /**
     * Only when it can be told: the suffix is `opus`, or `ogg` with a content type that names Opus
     * (a bare `audio/ogg` may as well be Vorbis).
     */
    val isOpus: Boolean
        get() = when (suffix?.lowercase()) {
            "opus" -> true
            "ogg", "oga" -> contentType?.lowercase()?.contains("opus") == true
            else -> false
        }

    companion object {
        fun of(song: Song): SourceFile = SourceFile(song.suffix, song.contentType, song.bitRate)
    }
}

/** The two quality settings: one for unmetered networks (Wi-Fi, Ethernet), one for metered. */
data class QualitySettings(
    val unmetered: StreamQuality = StreamQuality.DEFAULT_WIFI,
    val metered: StreamQuality = StreamQuality.DEFAULT_MOBILE,
) {
    fun forNetwork(metered: Boolean): StreamQuality = if (metered) this.metered else unmetered
}

/** What the device's network looks like when a track is loaded. */
enum class NetworkKind { Unmetered, Metered, None }

/**
 * Cache keys are by song id and quality, never by URL: the URL carries the token (it must not end
 * up in the cache index), and a song is the same bytes whatever URL fetched it.
 */
object CacheKeys {
    private const val PREFIX = "song/"

    fun song(songId: String, quality: StreamQuality): String = "$PREFIX$songId/${quality.id}"

    /** The song id back from a key, or null for a key that isn't a song's. */
    fun songId(key: String): String? =
        if (key.startsWith(PREFIX)) key.removePrefix(PREFIX).substringBeforeLast('/', "").ifEmpty { null } else null
}

/**
 * Picks the copy of a song to play, decided once per track when it is loaded:
 *
 * 1. A copy in the original quality that is fully cached wins on any network: it is the best
 *    there is and costs nothing to read. (A track cached in both qualities plays the original.)
 * 2. Otherwise the quality the network calls for; whatever part of it is cached is read from the
 *    cache and only the holes are fetched. For an Opus file that quality may be the original
 *    ([StreamQuality.forSource]: no live Opus-to-Opus transcode, which could not seek); then a
 *    fully cached copy in the quality chosen for the network, before the rule, still plays (it is
 *    complete and costs no data).
 * 3. With no network, any fully cached copy beats a stream that cannot start.
 *
 * A downloaded copy ([isDownloaded], complete in the downloads folder) comes before all of these,
 * in whatever quality it was downloaded: it is what the user asked to keep.
 */
object SourcePicker {
    fun pick(
        songId: String,
        network: NetworkKind,
        settings: QualitySettings,
        source: SourceFile? = null,
        isDownloaded: (key: String) -> Boolean = { false },
        isFullyCached: (key: String) -> Boolean,
    ): StreamQuality {
        StreamQuality.CHOICES.firstOrNull { isDownloaded(CacheKeys.song(songId, it)) }?.let { return it }
        if (isFullyCached(CacheKeys.song(songId, StreamQuality.ORIGINAL))) return StreamQuality.ORIGINAL
        val chosen = settings.forNetwork(metered = network != NetworkKind.Unmetered)
        val wanted = chosen.forSource(source)
        // The Opus rule sends the original, but a whole copy in the chosen quality is already here
        // (fetched before the rule, or while the file's details were unknown): it costs nothing.
        if (wanted != chosen && isFullyCached(CacheKeys.song(songId, chosen))) return chosen
        if (network == NetworkKind.None && !isFullyCached(CacheKeys.song(songId, wanted))) {
            (listOf(settings.unmetered, settings.metered) + StreamQuality.CHOICES)
                .firstOrNull { isFullyCached(CacheKeys.song(songId, it)) }
                ?.let { return it }
        }
        return wanted
    }
}

/** The stream URL for a song at a quality. It carries the token: never log it. */
fun SubsonicClient.streamUrl(songId: String, quality: StreamQuality): String =
    streamUrl(songId, format = quality.format, maxBitRate = quality.maxBitRate)

/**
 * Whether a transcode that ended cleanly is all there. A live transcode has no length: when the
 * server's encoder stops part way, the reply just ends, and the bytes so far look like a whole song.
 * The size a song should have follows from its length and the bitrate asked for; a copy under
 * [MIN_RATIO] of that (Opus is variable, so quiet music comes in under the nominal rate, but never by
 * that much) is taken as cut short.
 */
object TranscodeCheck {
    const val MIN_RATIO = 1.0 / 3

    /** Songs shorter than this aren't judged (a header's share of a few seconds is too large). */
    const val MIN_SECONDS = 20

    fun isCutShort(quality: StreamQuality, durationSeconds: Int?, bytes: Long): Boolean {
        if (quality.isOriginal) return false
        val kbps = quality.maxBitRate ?: return false
        val seconds = durationSeconds ?: return false
        if (seconds < MIN_SECONDS) return false
        val expected = seconds.toLong() * kbps * 1000 / 8
        return bytes < expected * MIN_RATIO
    }
}
