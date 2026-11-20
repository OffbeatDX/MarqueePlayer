package com.offbeat.marquee.core.auto

import com.offbeat.marquee.core.library.ShelfId
import java.net.URLDecoder
import java.net.URLEncoder

/**
 * The media ids of the browse tree that Android Auto (and any MediaBrowser) walks:
 *
 * ```
 * root
 * ├── Home ── one node per shelf ── albums ── tracks
 * ├── Playlists ── playlists ── tracks
 * ├── Recently played ── albums ── tracks
 * └── Downloads ── pinned albums and playlists (── tracks), then pinned songs
 * ```
 *
 * Every id starts with [PREFIX], so it can't be taken for a song id (queue items use the song id
 * itself). Server ids go in percent-encoded, so any character is safe. A track carries the list it
 * was shown in and its place there: playing it plays that list from it.
 */
sealed interface BrowseId {
    val id: String

    data object Root : BrowseId { override val id = "${PREFIX}root" }
    data object Home : BrowseId { override val id = "${PREFIX}home" }
    data object Playlists : BrowseId { override val id = "${PREFIX}playlists" }
    data object Recent : BrowseId { override val id = "${PREFIX}recent" }
    data object Downloads : BrowseId { override val id = "${PREFIX}downloads" }

    data class Shelf(val shelf: ShelfId) : BrowseId {
        override val id get() = "${PREFIX}shelf/${shelf.key}"
    }

    /** Browsable (its tracks) and playable (the whole album). */
    data class Album(val albumId: String) : BrowseId {
        override val id get() = "${PREFIX}album/${enc(albumId)}"
    }

    /** Browsable (its tracks) and playable (the whole playlist). */
    data class Playlist(val playlistId: String) : BrowseId {
        override val id get() = "${PREFIX}playlist/${enc(playlistId)}"
    }

    /** A song shown at [index] in [list]: plays [list] from it. */
    data class Track(val list: TrackList, val index: Int, val songId: String) : BrowseId {
        override val id get() = "${PREFIX}track/${list.kind}/${enc(list.listId)}/$index/${enc(songId)}"
    }

    companion object {
        const val PREFIX = "@m/"

        /** The browse id back from a media id; null for anything else (a song id, an unknown id). */
        fun parse(id: String?): BrowseId? {
            if (id == null || !id.startsWith(PREFIX)) return null
            val parts = id.removePrefix(PREFIX).split('/')
            return runCatching {
                when (parts[0]) {
                    "root" -> Root.takeIf { parts.size == 1 }
                    "home" -> Home.takeIf { parts.size == 1 }
                    "playlists" -> Playlists.takeIf { parts.size == 1 }
                    "recent" -> Recent.takeIf { parts.size == 1 }
                    "downloads" -> Downloads.takeIf { parts.size == 1 }
                    "shelf" -> if (parts.size == 2) ShelfId.fromKey(parts[1])?.let(::Shelf) else null
                    "album" -> if (parts.size == 2) dec(parts[1]).takeIf { it.isNotEmpty() }?.let(::Album) else null
                    "playlist" -> if (parts.size == 2) dec(parts[1]).takeIf { it.isNotEmpty() }?.let(::Playlist) else null
                    "track" -> if (parts.size == 5) {
                        val list = TrackList.of(parts[1], dec(parts[2]))
                        val index = parts[3].toInt()
                        val song = dec(parts[4])
                        if (list != null && index >= 0 && song.isNotEmpty()) Track(list, index, song) else null
                    } else null
                    else -> null
                }
            }.getOrNull()
        }

        // The String charset overloads: the Charset ones are Java 10, on Android only from API 33
        // (minSdk is 31), and crashed Android 12 with NoSuchMethodError.
        internal fun enc(s: String): String = URLEncoder.encode(s, "UTF-8").replace("+", "%20")
        internal fun dec(s: String): String = URLDecoder.decode(s, "UTF-8")
    }
}

/** The list a track was shown in, which plays when the track is picked. */
sealed interface TrackList {
    val kind: String
    val listId: String

    data class Album(val albumId: String) : TrackList {
        override val kind get() = "album"
        override val listId get() = albumId
    }

    data class Playlist(val playlistId: String) : TrackList {
        override val kind get() = "playlist"
        override val listId get() = playlistId
    }

    /** The songs pinned one by one (Downloads). */
    data object DownloadedSongs : TrackList {
        override val kind = "downloads"
        override val listId = "-"
    }

    /** The songs of a search; [query] is the search itself, so it can be asked again. */
    data class Search(val query: String) : TrackList {
        override val kind get() = "search"
        override val listId get() = query
    }

    companion object {
        fun of(kind: String, id: String): TrackList? = when (kind) {
            "album" -> Album(id).takeIf { id.isNotEmpty() }
            "playlist" -> Playlist(id).takeIf { id.isNotEmpty() }
            "downloads" -> DownloadedSongs
            "search" -> Search(id)
            else -> null
        }
    }
}

object BrowsePaging {
    /**
     * One page of [list]. Media3 asks page 0 with [Int.MAX_VALUE] for "everything" (a browser that
     * does not page), so the offset is computed without overflowing.
     */
    fun <T> page(list: List<T>, page: Int, pageSize: Int): List<T> {
        if (page < 0 || pageSize <= 0) return emptyList()
        val from = page.toLong() * pageSize
        if (from >= list.size) return emptyList()
        val to = minOf(list.size.toLong(), from + pageSize).toInt()
        return list.subList(from.toInt(), to)
    }

    /**
     * Where a picked track starts in its list as fetched now: its index when the song is still there,
     * otherwise where the song moved to (a playlist edited since), otherwise the top.
     */
    fun startIndex(songIds: List<String>, index: Int, songId: String): Int {
        if (songIds.getOrNull(index) == songId) return index
        val moved = songIds.indexOf(songId)
        return if (moved >= 0) moved else 0
    }
}
