package com.offbeat.marquee.player.playback

import android.os.Bundle
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import com.offbeat.marquee.core.art.CoverSize
import com.offbeat.marquee.player.art.Covers
import com.offbeat.marquee.core.playback.QueueEntry
import com.offbeat.marquee.core.playback.forQueue
import com.offbeat.marquee.core.subsonic.Song
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.json.Json

/**
 * Songs as media items and back. The item's id is the song id, its URI the token-free
 * placeholder, its artwork the cover provider's content URI (no token either: other apps load it), and the song itself (trimmed, see [forQueue]) rides along in the metadata extras,
 * so the service has the ReplayGain values and album order of every item without asking the server.
 */
object SongItems {
    private const val EXTRA_SONG = "marquee.song"
    private const val EXTRA_UID = "marquee.uid"
    private val uids = java.util.concurrent.atomic.AtomicLong(System.nanoTime())
    private val json = Json { ignoreUnknownKeys = true }

    /** The cover size asked for the notification and lock screen (a size bucket, shared with album headers). */
    const val NOTIFICATION_COVER_PX = CoverSize.HEADER

    fun toMediaItem(song: Song): MediaItem {
        val s = song.forQueue()
        val extras = Bundle().apply {
            putString(EXTRA_SONG, json.encodeToString(Song.serializer(), s))
            // Tells two queue entries of the same song apart (the queue sheet's keys).
            putLong(EXTRA_UID, uids.incrementAndGet())
        }
        val meta = MediaMetadata.Builder()
            .setTitle(s.title)
            .setArtist(s.artist)
            .setAlbumTitle(s.album)
            .setTrackNumber(s.track)
            .setDiscNumber(s.discNumber)
            .setDurationMs(s.duration?.let { it * 1000L })
            .setIsPlayable(true)
            .setIsBrowsable(false)
            .setMediaType(MediaMetadata.MEDIA_TYPE_MUSIC)
            .setArtworkUri(s.coverArt?.let { Covers.uri(it, NOTIFICATION_COVER_PX) })
            .setExtras(extras)
            .build()
        return MediaItem.Builder()
            .setMediaId(s.id)
            .setUri(SongSource.songUri(s))
            .setMediaMetadata(meta)
            .build()
    }

    /** The entry's own id in the queue (0 for an item made elsewhere). */
    fun uid(item: MediaItem): Long = item.mediaMetadata.extras?.getLong(EXTRA_UID) ?: 0L

    fun encodeSongs(songs: List<Song>): String = json.encodeToString(ListSerializer(Song.serializer()), songs.map { it.forQueue() })

    fun decodeSongs(text: String?): List<Song> =
        text?.let { runCatching { json.decodeFromString(ListSerializer(Song.serializer()), it) }.getOrNull() }.orEmpty()

    fun song(item: MediaItem): Song? {
        item.mediaMetadata.extras?.getString(EXTRA_SONG)?.let { text ->
            runCatching { json.decodeFromString(Song.serializer(), text) }.getOrNull()?.let { return it }
        }
        if (item.mediaId.isEmpty()) return null
        // An item from a controller that only knows the id and the visible metadata.
        val m = item.mediaMetadata
        return Song(
            id = item.mediaId,
            title = m.title?.toString().orEmpty(),
            artist = m.artist?.toString(),
            album = m.albumTitle?.toString(),
            track = m.trackNumber,
            discNumber = m.discNumber,
            duration = m.durationMs?.let { (it / 1000).toInt() },
        )
    }

    /** An item as a controller sent it (the URI is stripped on the way) made playable again. */
    fun resolve(item: MediaItem): MediaItem? = song(item)?.let(::toMediaItem)

    fun queueEntry(song: Song): QueueEntry = QueueEntry(song.albumId, song.discNumber, song.track)
}
