package com.offbeat.marquee.player.playback

import android.os.Bundle
import android.os.SystemClock
import com.offbeat.marquee.player.diag.AppLog
import android.util.LruCache
import androidx.annotation.OptIn
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.common.util.UnstableApi
import androidx.media3.session.MediaConstants
import com.offbeat.marquee.core.art.CoverSize
import com.offbeat.marquee.core.auto.BrowseId
import com.offbeat.marquee.core.auto.BrowsePaging
import com.offbeat.marquee.core.auto.OfflineSearch
import com.offbeat.marquee.core.auto.TrackList
import com.offbeat.marquee.core.library.ShelfId
import com.offbeat.marquee.core.library.Shelves
import com.offbeat.marquee.core.offline.Pin
import com.offbeat.marquee.core.offline.PinKind
import com.offbeat.marquee.core.offline.Playable
import com.offbeat.marquee.core.subsonic.Album
import com.offbeat.marquee.core.subsonic.Playlist
import com.offbeat.marquee.core.subsonic.Song
import com.offbeat.marquee.player.art.Covers
import com.offbeat.marquee.player.data.Library
import com.offbeat.marquee.player.net.NetworkMonitor
import com.offbeat.marquee.player.offline.Availability
import com.offbeat.marquee.player.offline.Downloads
import kotlin.coroutines.cancellation.CancellationException

/**
 * The browse tree for Android Auto and any other MediaBrowser ([BrowseId] has its shape), and search.
 *
 * Fast and frugal in the car: lists come from the app's response cache where it has them (album and
 * playlist pages, Home's shelves as Home last saw them) and are kept in memory for [TTL_MS], so
 * walking back and forth costs nothing; a page of a list is a slice of the list ([BrowsePaging]).
 * Offline, only what can play is listed (downloads, songs whole in the cache) and search looks at
 * the downloads only.
 *
 * Artwork is always the cover provider's content URI ([Covers]), never a URL with the token.
 */
@OptIn(UnstableApi::class)
class AutoLibrary {
    private class Entry(val atMs: Long, val items: List<MediaItem>)

    private val lists = LruCache<String, Entry>(48)
    private val searchSongs = LruCache<String, List<Song>>(8)
    private val searchItems = LruCache<String, Entry>(8)

    private val online get() = NetworkMonitor.online

    // ---- Nodes ---------------------------------------------------------------------------

    fun root(): MediaItem = folder(BrowseId.Root, "Marquee", MediaMetadata.MEDIA_TYPE_FOLDER_MIXED)

    /** The root's extras: tracks as lists, albums as a grid, unless a node says otherwise. */
    fun rootExtras(): Bundle = Bundle().apply {
        putInt(MediaConstants.EXTRAS_KEY_CONTENT_STYLE_BROWSABLE, MediaConstants.EXTRAS_VALUE_CONTENT_STYLE_GRID_ITEM)
        putInt(MediaConstants.EXTRAS_KEY_CONTENT_STYLE_PLAYABLE, MediaConstants.EXTRAS_VALUE_CONTENT_STYLE_LIST_ITEM)
    }

    /** The children of [parentId] (all of them; the caller slices a page), or null for an unknown id. */
    suspend fun children(parentId: String): List<MediaItem>? {
        val node = BrowseId.parse(parentId) ?: return null
        val key = "$parentId|${if (online) "on" else "off"}"
        lists.get(key)?.let { if (SystemClock.elapsedRealtime() - it.atMs < TTL_MS) return it.items }
        val items = load(node) ?: return null
        lists.put(key, Entry(SystemClock.elapsedRealtime(), items))
        AppLog.i(TAG, "children of ${describe(node)}: ${items.size}${if (online) "" else " (offline)"}")
        return items
    }

    /** One page of [parentId]'s children. */
    suspend fun page(parentId: String, page: Int, pageSize: Int): List<MediaItem>? =
        children(parentId)?.let { BrowsePaging.page(it, page, pageSize) }

    private suspend fun load(node: BrowseId): List<MediaItem>? = when (node) {
        BrowseId.Root -> listOf(
            folder(BrowseId.Home, "Home", MediaMetadata.MEDIA_TYPE_FOLDER_MIXED, browsableStyle = MediaConstants.EXTRAS_VALUE_CONTENT_STYLE_LIST_ITEM),
            folder(BrowseId.Playlists, "Playlists", MediaMetadata.MEDIA_TYPE_FOLDER_PLAYLISTS, browsableStyle = MediaConstants.EXTRAS_VALUE_CONTENT_STYLE_LIST_ITEM),
            folder(BrowseId.Recent, "Recently played", MediaMetadata.MEDIA_TYPE_FOLDER_ALBUMS),
            folder(BrowseId.Downloads, "Downloads", MediaMetadata.MEDIA_TYPE_FOLDER_MIXED),
        )
        BrowseId.Home -> com.offbeat.marquee.player.settings.UiSettings.shelves.value.visible.filter { it != ShelfId.RecentlyPlayed }.map {
            folder(BrowseId.Shelf(it), it.title, MediaMetadata.MEDIA_TYPE_FOLDER_ALBUMS)
        }
        is BrowseId.Shelf -> shelf(node.shelf).map(::albumItem)
        BrowseId.Recent -> shelf(ShelfId.RecentlyPlayed).map(::albumItem)
        is BrowseId.Album -> albumSongs(node.albumId)?.let { tracks(it, TrackList.Album(node.albumId)) }
        BrowseId.Playlists -> playlists().map(::playlistItem)
        is BrowseId.Playlist -> playlistSongs(node.playlistId)?.let { tracks(it, TrackList.Playlist(node.playlistId)) }
        BrowseId.Downloads -> {
            val pins = Downloads.pins.value.pins
            pins.filter { it.kind == PinKind.Album }.map { albumItem(Downloads.albumOf(it)) } +
                pins.filter { it.kind == PinKind.Playlist }.map { playlistItem(Downloads.playlistOf(it)) } +
                tracks(pinnedSongs(pins), TrackList.DownloadedSongs)
        }
        is BrowseId.Track -> null
    }

    /** One item by id (Auto asks for the item it is about to show or play). */
    suspend fun item(id: String): MediaItem? = when (val node = BrowseId.parse(id)) {
        null -> null
        is BrowseId.Album -> catching { Library.album(node.albumId) }.getOrNull()?.let(::albumItem)
        is BrowseId.Playlist -> catching { Library.playlist(node.playlistId) }.getOrNull()?.let(::playlistItem)
        is BrowseId.Track -> songsOf(node.list)?.let { songs ->
            val i = BrowsePaging.startIndex(songs.map { it.id }, node.index, node.songId)
            songs.getOrNull(i)?.takeIf { it.id == node.songId }?.let { trackItem(it, node.list, i) }
        }
        BrowseId.Root -> root()
        else -> children(BrowseId.Root.id)?.firstOrNull { it.mediaId == id } ?: children(BrowseId.Home.id)?.firstOrNull { it.mediaId == id }
    }

    // ---- Playing a browse item -----------------------------------------------------------

    /**
     * What picking [id] plays: an album or playlist from the top, a track in its list from it. Offline,
     * only what can play (null when nothing can). Null for an id that isn't playable.
     */
    suspend fun queueFor(id: String): Pair<List<Song>, Int>? {
        val node = BrowseId.parse(id) ?: return null
        val (songs, start) = when (node) {
            is BrowseId.Album -> (albumSongs(node.albumId) ?: return null) to 0
            is BrowseId.Playlist -> (playlistSongs(node.playlistId) ?: return null) to 0
            is BrowseId.Track -> {
                val songs = songsOf(node.list) ?: return null
                songs to BrowsePaging.startIndex(songs.map { it.id }, node.index, node.songId)
            }
            else -> return null
        }
        if (songs.isEmpty()) return null
        if (online) return songs to start
        return Playable.queueFrom(songs, start) { Availability.canPlayOffline(it.id) }
    }

    /** A spoken "play …": the songs of a search, or the first album's. Null when nothing matches. */
    suspend fun queueForSearch(query: String): List<Song>? {
        val songs = searchSongsFor(query)
        if (songs.isNotEmpty()) return songs
        val albumId = search(query).firstNotNullOfOrNull { (BrowseId.parse(it.mediaId) as? BrowseId.Album)?.albumId } ?: return null
        return albumSongs(albumId)?.takeIf { it.isNotEmpty() }
    }

    // ---- Search ------------------------------------------------------------------------------

    /** Albums (and, offline, pinned playlists) then songs; the songs play in the results' order. */
    suspend fun search(query: String): List<MediaItem> {
        val q = query.trim()
        if (q.isEmpty()) return emptyList()
        val key = "$q|${if (online) "on" else "off"}"
        searchItems.get(key)?.let { if (SystemClock.elapsedRealtime() - it.atMs < TTL_MS) return it.items }
        return searchNow(q).also { searchItems.put(key, Entry(SystemClock.elapsedRealtime(), it)) }
    }

    private suspend fun searchNow(q: String): List<MediaItem> {
        val albums: List<MediaItem>
        val songs: List<Song>
        if (online) {
            val r = catching { Library.search(q, artists = 0, albums = SEARCH_ALBUMS, songs = SEARCH_SONGS) }.getOrElse {
                AppLog.i(TAG, "search failed (${it.javaClass.simpleName}), downloads only")
                return offlineSearch(q)
            }
            albums = r.album.map(::albumItem)
            songs = r.song
        } else {
            return offlineSearch(q)
        }
        searchSongs.put(q, songs)
        AppLog.i(TAG, "search: ${albums.size} albums, ${songs.size} songs")
        return grouped(albums, "Albums") + grouped(tracks(songs, TrackList.Search(q)), "Songs")
    }

    private fun offlineSearch(q: String): List<MediaItem> {
        val r = OfflineSearch.search(Downloads.pins.value.pins, q, SEARCH_SONGS)
        searchSongs.put(q, r.songs)
        AppLog.i(TAG, "search (downloads only): ${r.collections.size} albums and playlists, ${r.songs.size} songs")
        val collections = r.collections.map { if (it.kind == PinKind.Album) albumItem(Downloads.albumOf(it)) else playlistItem(Downloads.playlistOf(it)) }
        return grouped(collections, "Downloads") + grouped(tracks(r.songs, TrackList.Search(q)), "Songs")
    }

    private suspend fun searchSongsFor(query: String): List<Song> {
        val q = query.trim()
        if (q.isEmpty()) return emptyList()
        searchSongs.get(q)?.let { return it }
        search(q)
        return searchSongs.get(q).orEmpty()
    }

    // ---- Lists ---------------------------------------------------------------------------

    private suspend fun songsOf(list: TrackList): List<Song>? = when (list) {
        is TrackList.Album -> albumSongs(list.albumId)
        is TrackList.Playlist -> playlistSongs(list.playlistId)
        TrackList.DownloadedSongs -> pinnedSongs(Downloads.pins.value.pins)
        is TrackList.Search -> searchSongsFor(list.query)
    }

    private fun pinnedSongs(pins: List<Pin>): List<Song> = pins.filter { it.kind == PinKind.Song }.flatMap { it.songs }

    /** A shelf as Home saved it, refreshed from the server when online (random albums keep their order). */
    private suspend fun shelf(id: ShelfId): List<Album> {
        val cached = Library.cachedShelf(id)
        val albums = if (!online || (id.isRandom && cached != null)) {
            cached.orEmpty()
        } else {
            catching { Library.fetchShelf(id) }.getOrElse { cached.orEmpty() }
        }
        return if (online) albums else albums.filter { a -> playableOffline(a) }
    }

    private suspend fun playableOffline(a: Album): Boolean =
        Downloads.isPinned(PinKind.Album, a.id) || Library.cachedAlbum(a.id)?.song?.any { Availability.canPlayOffline(it.id) } == true

    /** The album from the response cache, else the server. */
    private suspend fun albumSongs(id: String): List<Song>? =
        catching { Library.album(id).song }.getOrNull()?.let(::offlineFilter)

    /** Playlists change: from the server when online (the cache when that fails), the pinned ones offline. */
    private suspend fun playlists(): List<Playlist> =
        if (online) catching { Library.fetchPlaylists() }.getOrElse { Library.cachedPlaylists().orEmpty() }
        else Downloads.pins.value.pins.filter { it.kind == PinKind.Playlist }.map(Downloads::playlistOf)

    private suspend fun playlistSongs(id: String): List<Song>? {
        val p = if (online) catching { Library.fetchPlaylist(id) }.getOrNull() ?: Library.cachedPlaylist(id) else Library.cachedPlaylist(id)
        return p?.entry?.let(::offlineFilter)
    }

    private fun offlineFilter(songs: List<Song>): List<Song> = if (online) songs else songs.filter { Availability.canPlayOffline(it.id) }

    // ---- Items ---------------------------------------------------------------------------

    private fun folder(id: BrowseId, title: String, type: Int, browsableStyle: Int? = null): MediaItem {
        val extras = Bundle().apply {
            browsableStyle?.let { putInt(MediaConstants.EXTRAS_KEY_CONTENT_STYLE_BROWSABLE, it) }
        }
        return MediaItem.Builder()
            .setMediaId(id.id)
            .setMediaMetadata(
                MediaMetadata.Builder()
                    .setTitle(title)
                    .setIsBrowsable(true)
                    .setIsPlayable(false)
                    .setMediaType(type)
                    .setExtras(extras)
                    .build(),
            )
            .build()
    }

    private fun albumItem(a: Album): MediaItem = MediaItem.Builder()
        .setMediaId(BrowseId.Album(a.id).id)
        .setMediaMetadata(
            MediaMetadata.Builder()
                .setTitle(a.name)
                .setSubtitle(a.artist)
                .setArtist(a.artist)
                .setAlbumTitle(a.name)
                .setReleaseYear(a.year)
                .setIsBrowsable(true)
                .setIsPlayable(true)
                .setMediaType(MediaMetadata.MEDIA_TYPE_ALBUM)
                .setArtworkUri(a.coverArt?.let { Covers.uri(it, CoverSize.GRID) })
                .setExtras(downloadExtras(Downloads.isPinned(PinKind.Album, a.id)))
                .build(),
        )
        .build()

    private fun playlistItem(p: Playlist): MediaItem = MediaItem.Builder()
        .setMediaId(BrowseId.Playlist(p.id).id)
        .setMediaMetadata(
            MediaMetadata.Builder()
                .setTitle(p.name)
                .setSubtitle(if (p.songCount == 1) "1 song" else "${p.songCount} songs")
                .setIsBrowsable(true)
                .setIsPlayable(true)
                .setMediaType(MediaMetadata.MEDIA_TYPE_PLAYLIST)
                .setArtworkUri(p.coverArt?.let { Covers.uri(it, CoverSize.GRID) })
                .setExtras(downloadExtras(Downloads.isPinned(PinKind.Playlist, p.id)))
                .build(),
        )
        .build()

    private fun tracks(songs: List<Song>, list: TrackList): List<MediaItem> = songs.mapIndexed { i, s -> trackItem(s, list, i) }

    private fun trackItem(s: Song, list: TrackList, index: Int): MediaItem = MediaItem.Builder()
        .setMediaId(BrowseId.Track(list, index, s.id).id)
        .setMediaMetadata(
            MediaMetadata.Builder()
                .setTitle(s.title)
                .setSubtitle(s.artist)
                .setArtist(s.artist)
                .setAlbumTitle(s.album)
                .setTrackNumber(s.track)
                .setDurationMs(s.duration?.let { it * 1000L })
                .setIsBrowsable(false)
                .setIsPlayable(true)
                .setMediaType(MediaMetadata.MEDIA_TYPE_MUSIC)
                .setArtworkUri(s.coverArt?.let { Covers.uri(it, CoverSize.GRID) })
                .setExtras(downloadExtras(Downloads.isDownloaded(s.id)))
                .build(),
        )
        .build()

    private fun downloadExtras(downloaded: Boolean) = Bundle().apply {
        if (downloaded) putLong(MediaConstants.EXTRAS_KEY_DOWNLOAD_STATUS, MediaConstants.EXTRAS_VALUE_STATUS_DOWNLOADED)
    }

    /** Search results under a heading (Auto shows the group title above them). */
    private fun grouped(items: List<MediaItem>, title: String): List<MediaItem> = items.map { item ->
        val extras = Bundle(item.mediaMetadata.extras ?: Bundle.EMPTY).apply {
            putString(MediaConstants.EXTRAS_KEY_CONTENT_STYLE_GROUP_TITLE, title)
        }
        item.buildUpon().setMediaMetadata(item.mediaMetadata.buildUpon().setExtras(extras).build()).build()
    }

    /** Forgets the lists kept in memory (sign out, a different server). */
    fun clear() {
        lists.evictAll()
        searchSongs.evictAll()
        searchItems.evictAll()
    }

    /** A node for the log: its kind, no names. */
    private fun describe(node: BrowseId): String = when (node) {
        is BrowseId.Shelf -> "shelf ${node.shelf.key}"
        is BrowseId.Album -> "album ${node.albumId}"
        is BrowseId.Playlist -> "playlist ${node.playlistId}"
        else -> node.id.removePrefix(BrowseId.PREFIX)
    }

    private companion object {
        const val TAG = "MarqueeAuto"
        const val TTL_MS = 10 * 60_000L
        const val SEARCH_ALBUMS = 10
        const val SEARCH_SONGS = 40
    }
}

/**
 * runCatching that lets a cancellation through: when the service goes (its scope cancelled), a
 * request stops there rather than falling back to the cache and carrying on.
 */
internal inline fun <T> catching(block: () -> T): Result<T> =
    runCatching(block).onFailure { if (it is CancellationException) throw it }
