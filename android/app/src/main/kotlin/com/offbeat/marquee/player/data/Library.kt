package com.offbeat.marquee.player.data

import com.offbeat.marquee.player.BuildConfig
import android.content.Context
import com.offbeat.marquee.player.diag.AppLog
import com.offbeat.marquee.core.library.AlbumPaging
import com.offbeat.marquee.core.library.AlbumSort
import com.offbeat.marquee.core.library.CountSearch
import com.offbeat.marquee.core.library.DiskStore
import com.offbeat.marquee.core.library.ItemKind
import com.offbeat.marquee.core.library.ShelfId
import com.offbeat.marquee.core.library.Shelves
import com.offbeat.marquee.core.subsonic.Album
import com.offbeat.marquee.core.subsonic.Artist
import com.offbeat.marquee.core.subsonic.ArtistsIndex
import com.offbeat.marquee.core.subsonic.Genre
import com.offbeat.marquee.core.subsonic.Playlist
import com.offbeat.marquee.core.subsonic.ReadOnlyPolicy
import com.offbeat.marquee.core.subsonic.SearchResult
import com.offbeat.marquee.core.subsonic.Song
import com.offbeat.marquee.core.subsonic.Starred
import com.offbeat.marquee.core.subsonic.SubsonicClient
import com.offbeat.marquee.core.subsonic.SubsonicError
import com.offbeat.marquee.core.sync.DirectWrite
import com.offbeat.marquee.core.sync.sendOrQueue
import com.offbeat.marquee.player.session.AppSession
import com.offbeat.marquee.player.net.NetworkMonitor
import com.offbeat.marquee.player.offline.Downloads
import com.offbeat.marquee.player.sync.Sync
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withPermit
import kotlinx.coroutines.withContext
import kotlinx.serialization.KSerializer
import kotlinx.serialization.Serializable
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.builtins.serializer
import kotlinx.serialization.json.Json
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import java.io.File
import java.util.Calendar

/** An artist page: the artist with its albums, the image to show, and its top songs. */
@Serializable
data class ArtistPage(
    val artist: Artist,
    /** A cover id for `getCoverArt` when the server has an image of the artist; null: use blurred album art. */
    val imageCoverId: String? = null,
    val top: List<Song> = emptyList(),
)

/** What a long press, a play or a write is about. */
sealed interface Item {
    val id: String
    val kind: ItemKind

    data class OfAlbum(val album: Album) : Item {
        override val id get() = album.id
        override val kind get() = ItemKind.Album
    }

    data class OfSong(val song: Song) : Item {
        override val id get() = song.id
        override val kind get() = ItemKind.Song
    }

    data class OfArtist(val artist: Artist) : Item {
        override val id get() = artist.id
        override val kind get() = ItemKind.Artist
    }

    data class OfPlaylist(val playlist: Playlist) : Item {
        override val id get() = playlist.id
        override val kind get() = ItemKind.Playlist
    }
}

/**
 * The app's one data layer over the Subsonic client. The last good answers are kept on disk
 * (Home's shelves, the Library's lists and first pages, and the album, artist and playlist pages
 * opened before, least recently used out), so a screen shows them at once and then refreshes.
 *
 * Favourites changed here are shown at once ([favourites]) and rolled back if the server refuses
 * them; read-only builds refuse them before anything changes.
 */
object Library {
    private const val TAG = "MarqueeLibrary"
    private lateinit var app: Context
    private val json = Json { ignoreUnknownKeys = true; encodeDefaults = false }

    private val home by lazy { DiskStore(File(app.filesDir, "responses/home"), 32) }
    private val albums by lazy { DiskStore(File(app.filesDir, "responses/albums"), 60) }
    private val artists by lazy { DiskStore(File(app.filesDir, "responses/artists"), 30) }
    private val playlists by lazy { DiskStore(File(app.filesDir, "responses/playlists"), 30) }
    private val lists by lazy { DiskStore(File(app.filesDir, "responses/lists"), 40) }

    private val albumList = ListSerializer(Album.serializer())

    fun init(context: Context) {
        app = context.applicationContext
        favouriteStates.load()
    }

    private fun client(): SubsonicClient =
        AppSession.current ?: throw SubsonicError.Network(SubsonicError.Network.Kind.Other, null)

    private suspend fun <T> read(store: DiskStore, key: String, serializer: KSerializer<T>): T? = withContext(Dispatchers.IO) {
        store.read(key)?.let { text -> runCatching { json.decodeFromString(serializer, text) }.getOrNull() }
    }

    private suspend fun <T> write(store: DiskStore, key: String, serializer: KSerializer<T>, value: T) = withContext(Dispatchers.IO) {
        store.write(key, json.encodeToString(serializer, value))
    }

    private fun year() = Calendar.getInstance().get(Calendar.YEAR)

    // ---- Home ------------------------------------------------------------------------------

    suspend fun cachedShelf(id: ShelfId): List<Album>? = read(home, id.key, albumList)

    /** A page of a shelf. The shelf's own first page (what Home shows) is kept on disk. */
    suspend fun fetchShelf(id: ShelfId, size: Int = Shelves.shelfSize(id), offset: Int = 0): List<Album> {
        val q = Shelves.query(id, size, offset, year())
        val list = client().getAlbumList2(q.type, size = q.size, offset = q.offset, fromYear = q.fromYear, toYear = q.toYear)
        if (offset == 0 && size == Shelves.shelfSize(id)) write(home, id.key, albumList, list)
        AppLog.i(TAG, "shelf ${id.key}: ${list.size} albums at offset $offset")
        return list
    }

    // ---- Library: albums by sort -------------------------------------------------------------

    private fun pageKey(sort: AlbumSort) = "albums-${sort.key}-0"
    private fun countKey(sort: AlbumSort) = "count-${sort.key}"

    /** The sort's first page as last seen, for an instant first screen. */
    suspend fun cachedAlbumPage(sort: AlbumSort): List<Album>? = read(lists, pageKey(sort), albumList)

    /** A page of [AlbumPaging.PAGE_SIZE] albums in [sort]'s order; the first page is kept on disk. */
    suspend fun fetchAlbumPage(sort: AlbumSort, page: Int): List<Album> {
        val q = sort.query(AlbumPaging.PAGE_SIZE, AlbumPaging.offsetOf(page), year())
        val list = client().getAlbumList2(q.type, size = q.size, offset = q.offset, fromYear = q.fromYear, toYear = q.toYear)
        if (page == 0 && sort.pagesByOffset) write(lists, pageKey(sort), albumList, list)
        AppLog.i(TAG, "albums ${sort.key}: page $page, ${list.size} albums")
        return list
    }

    suspend fun cachedAlbumCount(sort: AlbumSort): Int? = read(lists, countKey(sort), Int.serializer())

    /**
     * How many albums [sort] lists. `getScanStatus` counts files, not albums, and `getAlbumList2`
     * has no total, so it is probed with one-album requests ([CountSearch]): two to confirm last
     * time's count, about a dozen the first time.
     */
    suspend fun albumCount(sort: AlbumSort): Int {
        val c = client()
        val q = sort.query(1, 0, year())
        var probes = 0
        val n = CountSearch.count(cachedAlbumCount(sort)) { offset ->
            probes++
            c.getAlbumList2(q.type, size = 1, offset = offset, fromYear = q.fromYear, toYear = q.toYear).isNotEmpty()
        }
        write(lists, countKey(sort), Int.serializer(), n)
        AppLog.i(TAG, "albums ${sort.key}: $n in all ($probes probes)")
        return n
    }

    // ---- Albums ----------------------------------------------------------------------------

    /** The album as last fetched, or as pinned for offline. */
    suspend fun cachedAlbum(id: String): Album? = read(albums, id, Album.serializer()) ?: Downloads.pinnedAlbum(id)

    suspend fun fetchAlbum(id: String): Album = client().getAlbum(id).also { write(albums, id, Album.serializer(), it) }

    /** The album from the disk cache when it is there, otherwise from the server. */
    suspend fun album(id: String): Album = cachedAlbum(id) ?: fetchAlbum(id)

    // ---- Artists ---------------------------------------------------------------------------

    suspend fun cachedArtists(): ArtistsIndex? = read(lists, "artists", ArtistsIndex.serializer())

    suspend fun fetchArtists(): ArtistsIndex = client().getArtists().also {
        write(lists, "artists", ArtistsIndex.serializer(), it)
        AppLog.i(TAG, "artists: ${it.all.size} in ${it.index.size} letters")
    }

    suspend fun cachedArtist(id: String): ArtistPage? = read(artists, id, ArtistPage.serializer())

    suspend fun fetchArtist(id: String): ArtistPage = coroutineScope {
        val c = client()
        val info = async { runCatching { c.getArtistInfo2(id) }.getOrNull() }
        val artist = c.getArtist(id)
        val top = async { runCatching { c.getTopSongs(artist.name, 50) }.getOrDefault(emptyList()) }
        val page = ArtistPage(artist, imageCoverIdFor(c, artist, info.await()?.largeImageUrl), top.await())
        write(artists, id, ArtistPage.serializer(), page)
        page
    }

    suspend fun artist(id: String): ArtistPage = cachedArtist(id) ?: fetchArtist(id)

    /**
     * The artist's picture, only from the server itself: Navidrome's `largeImageUrl` is its own
     * `/share/img/<token for ar-…>` link, the same picture `getCoverArt` gives for the artist's
     * cover id, which goes through the authenticated (and in debug read-only) client. An image URL
     * on any other host is never fetched: the page falls back to blurred album art.
     */
    private fun imageCoverIdFor(c: SubsonicClient, artist: Artist, largeImageUrl: String?): String? {
        val coverId = artist.coverArt ?: return null
        val image = largeImageUrl?.toHttpUrlOrNull() ?: return null
        val server = SubsonicClient.normalizeUrl(c.credentials.url) ?: return null
        return coverId.takeIf { image.host.equals(server.host, ignoreCase = true) }
    }

    /**
     * Every song of the artist's albums: at most [maxAlbums] albums and [maxSongs] songs, fetched
     * four at a time, from the cache where possible. In album order (oldest first), or shuffled.
     */
    suspend fun artistSongs(page: ArtistPage, shuffled: Boolean, maxAlbums: Int = 40, maxSongs: Int = 500): List<Song> = coroutineScope {
        val gate = Semaphore(4)
        val picked = page.artist.album.sortedByDescending { it.year ?: 0 }.take(maxAlbums).sortedWith(compareBy({ it.year ?: 0 }, { it.name }))
        val songs = picked.map { a ->
            async { gate.withPermit { runCatching { album(a.id).song }.getOrDefault(emptyList()) } }
        }.awaitAll().flatten()
        (if (shuffled) songs.shuffled() else songs).take(maxSongs)
    }

    /** "Shuffle all" on an artist page. */
    suspend fun shuffledSongs(page: ArtistPage): List<Song> = artistSongs(page, shuffled = true)

    // ---- Playlists -------------------------------------------------------------------------

    suspend fun cachedPlaylists(): List<Playlist>? = read(lists, "playlists", ListSerializer(Playlist.serializer()))

    suspend fun fetchPlaylists(): List<Playlist> = client().getPlaylists().also {
        write(lists, "playlists", ListSerializer(Playlist.serializer()), it)
        _playlistsChanged.value = it
    }

    private val _playlistsChanged = MutableStateFlow<List<Playlist>?>(null)

    /** The playlists as last fetched or changed here (an addition bumps a count before the server answers). */
    val playlistsChanged: StateFlow<List<Playlist>?> = _playlistsChanged.asStateFlow()

    suspend fun cachedPlaylist(id: String): Playlist? = read(playlists, id, Playlist.serializer()) ?: Downloads.pinnedPlaylist(id)

    /** A playlist from the server; a pinned one follows what it now lists ([Downloads.onPlaylistFetched]). */
    suspend fun fetchPlaylist(id: String): Playlist = client().getPlaylist(id).also {
        write(playlists, id, Playlist.serializer(), it)
        Downloads.onPlaylistFetched(it)
    }

    suspend fun playlist(id: String): Playlist = cachedPlaylist(id) ?: fetchPlaylist(id)

    /** Playlists songs can be added to: the user's own, not smart (read-only) ones. The owner is never shown. */
    fun editable(list: List<Playlist>): List<Playlist> {
        val user = AppSession.current?.credentials?.user
        return list.filter { it.readonly != true && (it.owner == null || it.owner == user) }
    }

    // ---- Genres ------------------------------------------------------------------------------

    suspend fun cachedGenres(): List<Genre>? = read(lists, "genres", ListSerializer(Genre.serializer()))

    suspend fun fetchGenres(): List<Genre> = client().getGenres().also {
        write(lists, "genres", ListSerializer(Genre.serializer()), it)
        AppLog.i(TAG, "genres: ${it.size}")
    }

    suspend fun genreAlbums(genre: String, offset: Int, size: Int = AlbumPaging.PAGE_SIZE): List<Album> =
        client().getAlbumList2(com.offbeat.marquee.core.subsonic.AlbumListType.ByGenre, size = size, offset = offset, genre = genre)

    /** Up to [max] songs of a genre, shuffled ("Shuffle genre"). */
    suspend fun genreSongs(genre: String, max: Int = 300): List<Song> =
        client().getSongsByGenre(genre, count = max).shuffled()

    // ---- Favourites (getStarred2) ------------------------------------------------------------

    suspend fun cachedStarred(): Starred? = read(lists, "starred", Starred.serializer())

    suspend fun fetchStarred(): Starred = client().getStarred2().also {
        write(lists, "starred", Starred.serializer(), it)
        followServerFavourites(it)
        AppLog.i(TAG, "starred: ${it.artist.size} artists, ${it.album.size} albums, ${it.song.size} songs")
    }

    // ---- Search ------------------------------------------------------------------------------

    suspend fun search(
        query: String,
        artists: Int,
        albums: Int,
        songs: Int,
        artistOffset: Int = 0,
        albumOffset: Int = 0,
        songOffset: Int = 0,
    ): SearchResult = client().search3(query, artists, albums, songs, artistOffset, albumOffset, songOffset)

    // ---- Songs of an item (play, play next, add to queue) ------------------------------------

    suspend fun songsOf(item: Item, shuffled: Boolean = false): List<Song> {
        val songs = when (item) {
            is Item.OfSong -> listOf(item.song)
            is Item.OfAlbum -> album(item.album.id).song
            is Item.OfPlaylist -> playlist(item.playlist.id).entry
            is Item.OfArtist -> return artistSongs(artist(item.artist.id), shuffled)
        }
        return if (shuffled) songs.shuffled() else songs
    }

    // ---- Messages ----------------------------------------------------------------------------

    private val _messages = MutableSharedFlow<String>(extraBufferCapacity = 4)

    /** Short notes for a snackbar ("Read-only test build"). */
    val messages: SharedFlow<String> = _messages.asSharedFlow()

    fun post(message: String) {
        _messages.tryEmit(message)
    }

    /**
     * Test builds only: the note that a write was refused. Gated on the build-time constant, so R8
     * drops it (and every read-only branch below) from the release build, which is read-write.
     */
    fun postReadOnly() {
        if (BuildConfig.READ_ONLY) post("Read-only test build")
    }

    // ---- Writes: favourites, playlist additions -------------------------------------

    // Kept on disk, so the hearts (the notification's too) are right after a restart, offline
    // included. Read and written on one background thread of their own.
    @OptIn(ExperimentalCoroutinesApi::class)
    private val favouriteStates = FavouritesStore(
        file = { File(app.filesDir, "favourites.json") },
        scope = CoroutineScope(SupervisorJob() + Dispatchers.IO.limitedParallelism(1)),
    )

    /** Favourite states changed in this run, by id; anything else is as the server last said (`starred`). */
    val favourites: StateFlow<Map<String, Boolean>> = favouriteStates.state

    fun isFavourite(id: String, starred: String?, overrides: Map<String, Boolean> = favouriteStates.state.value): Boolean =
        overrides[id] ?: (starred != null)

    /**
     * A write to the server. In a read-only build nothing changes: the client refuses it locally
     * (logged, never sent) and the snackbar says so. Otherwise [apply] shows the change at once and
     * [undo] takes it back if the server says no. Returns whether the change stands.
     */
    private suspend fun <T> writeThrough(
        endpoint: String,
        what: String,
        apply: () -> T,
        undo: (T) -> Unit,
        call: suspend (SubsonicClient) -> Unit,
    ): Boolean {
        val c = AppSession.current ?: return false
        val refused = BuildConfig.READ_ONLY && c.readOnly && !ReadOnlyPolicy.isAllowed(endpoint)
        val ticket = if (refused) null else apply()
        return try {
            call(c)
            AppLog.i(TAG, "$what: done")
            true
        } catch (e: SubsonicError.RefusedReadOnly) {
            AppLog.w(TAG, "$what not changed: ${e.endpoint} refused (read-only build)")
            ticket?.let(undo)
            postReadOnly()
            false
        } catch (e: SubsonicError) {
            AppLog.w(TAG, "$what not changed: ${e.javaClass.simpleName}")
            ticket?.let(undo)
            post("Couldn't save that. Check the connection.")
            false
        }
    }

    /**
     * Stars or unstars a song, album or artist. Read-only builds refuse it (nothing changes). In a
     * normal build the heart changes at once and is kept on disk; when the write can't reach the
     * server (offline, or the network fails) it waits in the outbox ([Sync]) and the heart stays as
     * set. [report] posts the snackbar messages (the service passes false: its caller says them).
     */
    suspend fun setFavourite(item: Item, favourite: Boolean, report: Boolean = true): Boolean {
        val c = AppSession.current ?: return false
        val endpoint = if (favourite) "star" else "unstar"
        val what = "${item.kind.name.lowercase()} ${item.id} favourite=$favourite"
        if (BuildConfig.READ_ONLY && c.readOnly && !ReadOnlyPolicy.isAllowed(endpoint)) {
            try {
                starCall(c, item, favourite) // refused locally, never sent: logged as such
            } catch (e: SubsonicError.RefusedReadOnly) {
                AppLog.w(TAG, "$what not changed: ${e.endpoint} refused (read-only build)")
            }
            if (report) postReadOnly()
            return false
        }
        val ticket = favouriteStates.set(item.id, favourite)
        val kind = item.kind.name.lowercase()
        val outcome = sendOrQueue(
            online = NetworkMonitor.online,
            send = { starCall(c, item, favourite) },
            queue = { Sync.queueStar(kind, item.id, favourite) },
        )
        return when (outcome) {
            DirectWrite.Sent -> {
                AppLog.i(TAG, "$what: done")
                // Keep the saved album page in step, so it opens with the right heart next time.
                if (item.kind == ItemKind.Album) {
                    read(albums, item.id, Album.serializer())?.let { write(albums, item.id, Album.serializer(), it.copy(starred = if (favourite) "now" else null)) }
                }
                true
            }
            is DirectWrite.Queued -> {
                AppLog.i(TAG, "$what: ${outcome.error?.let { "not sent (${it.javaClass.simpleName})" } ?: "offline"}, queued")
                true
            }
            is DirectWrite.Refused -> {
                AppLog.w(TAG, "$what not changed: ${outcome.error.javaClass.simpleName}")
                favouriteStates.rollback(ticket)
                if (report) post("Couldn't save that. Check the connection.")
                false
            }
        }
    }

    private suspend fun starCall(c: SubsonicClient, item: Item, favourite: Boolean) {
        val (id, albumId, artistId) = when (item.kind) {
            ItemKind.Album -> Triple(null, item.id, null)
            ItemKind.Artist -> Triple(null, null, item.id)
            else -> Triple(item.id, null, null)
        }
        if (favourite) c.star(id, albumId, artistId) else c.unstar(id, albumId, artistId)
    }

    /**
     * getStarred2 is the server's whole answer: the states kept here follow it (a change made on
     * another device shows), except changes still waiting in the outbox.
     */
    private fun followServerFavourites(s: Starred) {
        val starred = HashSet<String>().apply {
            s.song.forEach { add(it.id) }
            s.album.forEach { add(it.id) }
            s.artist.forEach { add(it.id) }
        }
        val changes = HashMap<String, Boolean>()
        for ((id, v) in favouriteStates.snapshot()) {
            if (Sync.outbox.pendingStar(id) != null) continue
            val server = id in starred
            if (server != v) changes[id] = server
        }
        favouriteStates.setAll(changes)
    }

    /** Adds songs to the end of a playlist; its song count goes up at once. */
    suspend fun addToPlaylist(playlist: Playlist, songs: List<Song>): Boolean {
        if (songs.isEmpty()) return false
        val before = _playlistsChanged.value ?: cachedPlaylists()
        val ok = writeThrough(
            endpoint = "updatePlaylist",
            what = "playlist ${playlist.id} +${songs.size} songs",
            apply = {
                _playlistsChanged.value = before?.map {
                    if (it.id == playlist.id) it.copy(songCount = it.songCount + songs.size, duration = it.duration + songs.sumOf { s -> s.duration ?: 0 }) else it
                }
                before
            },
            undo = { old -> _playlistsChanged.value = old },
            call = { c -> c.updatePlaylist(playlist.id, songs.map { it.id }) },
        )
        if (ok) {
            playlists.remove(playlist.id)
            post("Added to ${playlist.name}")
        }
        return ok
    }

    /** Forgets every kept answer and change (sign out). */
    suspend fun clear() = withContext(Dispatchers.IO) {
        home.clear()
        albums.clear()
        artists.clear()
        playlists.clear()
        lists.clear()
        favouriteStates.clear()
        _playlistsChanged.value = null
    }
}

/**
 * Serializes atomic writes to `favourites.json` via `<name>.tmp` and skips any snapshot whose
 * sequence number is older than the last write or clear (sign-out), so a late write can't undo
 * a newer one or bring back a deleted file.
 */
internal class FavouritesWriter {
    private val seqGen = java.util.concurrent.atomic.AtomicLong(0)
    private var lastSeq = 0L

    fun nextSeq(): Long = seqGen.incrementAndGet()

    @Synchronized
    fun write(file: File, text: String, seq: Long = nextSeq()): Boolean {
        if (seq <= lastSeq) return false
        lastSeq = seq
        file.parentFile?.mkdirs()
        val tmp = File(file.parentFile, file.name + ".tmp")
        tmp.writeText(text)
        if (!tmp.renameTo(file)) {
            file.delete()
            if (!tmp.renameTo(file)) tmp.delete()
        }
        return true
    }

    /** Deletes [file] (and a leftover temp file) unless a newer write or clear already ran. */
    @Synchronized
    fun clear(file: File, seq: Long = nextSeq()): Boolean {
        if (seq <= lastSeq) return false
        lastSeq = seq
        File(file.parentFile, file.name + ".tmp").delete()
        file.delete()
        return true
    }
}
