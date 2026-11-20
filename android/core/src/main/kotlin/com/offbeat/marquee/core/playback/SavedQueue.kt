package com.offbeat.marquee.core.playback

import com.offbeat.marquee.core.subsonic.Song
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

/**
 * The queue as it is kept on the device between runs: the songs with just enough to show and
 * play them, the current index (in queue order) and the position in it.
 *
 * Not synced with the server: getPlayQueue / savePlayQueue would let the desktop and the phone
 * hand a queue over, but savePlayQueue is a write, so it would stay off in read-only builds.
 */
@Serializable
data class SavedQueue(
    val songs: List<Song>,
    val index: Int,
    val positionMs: Long,
    val shuffle: Boolean = false,
    val repeatMode: Int = 0,
    val savedAtMs: Long = 0,
    /** The play order (timeline indices) while shuffle is on, so edits made under shuffle survive a restart. */
    val shuffleOrder: List<Int>? = null,
) {
    /** [shuffleOrder] when it fits the songs, else null (the player then makes a new one). */
    val validShuffleOrder: List<Int>? get() = shuffleOrder?.takeIf { QueueOrder.isValid(it, songs.size) }

    fun encode(): String = json.encodeToString(serializer(), this)

    companion object {
        private val json = Json { ignoreUnknownKeys = true; encodeDefaults = false }

        fun decode(text: String): SavedQueue? = try {
            json.decodeFromString(serializer(), text).takeIf { it.songs.isNotEmpty() }?.let {
                it.copy(index = it.index.coerceIn(0, it.songs.lastIndex), positionMs = it.positionMs.coerceAtLeast(0))
            }
        } catch (e: Exception) {
            null
        }
    }
}

/**
 * The song with only what the queue and the player use, so the saved queue stays small. The file's
 * suffix, content type and bitrate stay: they decide whether an Opus file plays as it is
 * ([StreamQuality.forSource]).
 */
fun Song.forQueue(): Song = Song(
    id = id,
    title = title,
    album = album,
    albumId = albumId,
    artist = artist,
    artistId = artistId,
    track = track,
    discNumber = discNumber,
    coverArt = coverArt,
    duration = duration,
    suffix = suffix,
    contentType = contentType,
    bitRate = bitRate,
    starred = starred,
    replayGain = replayGain,
)
