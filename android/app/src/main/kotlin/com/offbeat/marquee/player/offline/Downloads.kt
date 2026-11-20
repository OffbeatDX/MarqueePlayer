package com.offbeat.marquee.player.offline

import android.content.Context
import com.offbeat.marquee.player.diag.AppLog
import androidx.compose.runtime.Immutable
import androidx.work.Constraints
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import com.offbeat.marquee.core.offline.ListDiff
import com.offbeat.marquee.core.offline.Pin
import com.offbeat.marquee.core.offline.PinChange
import com.offbeat.marquee.core.offline.PinKind
import com.offbeat.marquee.core.offline.PinSet
import com.offbeat.marquee.core.offline.Reconcile
import com.offbeat.marquee.core.playback.CacheKeys
import com.offbeat.marquee.core.playback.SourceFile
import com.offbeat.marquee.core.playback.StreamQuality
import com.offbeat.marquee.core.playback.forQueue
import com.offbeat.marquee.core.subsonic.Album
import com.offbeat.marquee.core.subsonic.Playlist
import com.offbeat.marquee.core.subsonic.Song
import com.offbeat.marquee.core.subsonic.SubsonicError
import com.offbeat.marquee.player.data.Item
import com.offbeat.marquee.player.data.Library
import com.offbeat.marquee.player.net.NetState
import com.offbeat.marquee.player.net.NetworkMonitor
import com.offbeat.marquee.player.playback.MediaStores
import com.offbeat.marquee.player.session.errorMessage
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.io.File
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.TimeUnit

/** What is on the phone: songs complete in the downloads folder, bytes per song, the folder's size. */
@Immutable
data class Stored(
    val complete: Set<String> = emptySet(),
    val bytes: Map<String, Long> = emptyMap(),
    val totalBytes: Long = 0,
    /** Songs the server would not give (gone, say): not retried until they are pinned again. */
    val failed: Set<String> = emptySet(),
)

/** The download worker right now: running or not, and the song it is on. */
@Immutable
data class Activity(val running: Boolean = false, val songId: String? = null, val bytes: Long = 0, val length: Long = -1)

/** How far a pin is, for its button, the Downloads screen and the notification. */
@Immutable
data class PinProgress(
    val total: Int,
    val done: Int,
    val bytes: Long,
    val fraction: Float,
    val state: State,
    val failed: Int = 0,
) {
    enum class State { Done, Downloading, WaitingForWifi, WaitingForNetwork, Queued }

    val complete: Boolean get() = state == State.Done

    val label: String get() = when (state) {
        State.Done -> if (failed > 0) "Downloaded, $failed couldn't be" else "Downloaded"
        State.Downloading -> "Downloading $done of $total"
        State.WaitingForWifi -> "Waiting for Wi-Fi"
        State.WaitingForNetwork -> "Waiting for a connection"
        State.Queued -> "Queued"
    }
}

/**
 * Pinned albums, playlists and songs, kept for offline. The pins live in a small JSON file; the
 * songs in their own never-evicted SimpleCache ([MediaStores.downloads]), stored once however many
 * pins share them and deleted when the last pin goes ([PinSet]). A [DownloadWorker] (WorkManager,
 * on an unmetered network when "Wi-Fi only" is on) fetches what is missing and resumes where it
 * stopped; pinned playlists follow the server ([syncPlaylists]: on start, when the playlist page
 * opens, and daily on Wi-Fi while charging, [PlaylistSyncWorker]). Their covers are kept with them
 * ([CoverStore]), outside the evictable cover cache.
 */
object Downloads {
    private const val TAG = "MarqueeDownloads"
    const val WORK = "downloads"
    const val SYNC_WORK = "downloads-playlist-sync"

    /** The qualities a download can be in (Settings > Downloads): the same five as streaming. */
    val QUALITIES: List<StreamQuality> = com.offbeat.marquee.core.settings.QualityChoices.OFFERED

    private lateinit var app: Context
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val mutex = Mutex()
    private var started = false

    private val _pins = MutableStateFlow(PinSet())
    val pins: StateFlow<PinSet> = _pins.asStateFlow()
    private val _stored = MutableStateFlow(Stored())
    val stored: StateFlow<Stored> = _stored.asStateFlow()
    private val _activity = MutableStateFlow(Activity())
    val activity: StateFlow<Activity> = _activity.asStateFlow()
    private val _quality = MutableStateFlow(StreamQuality.ORIGINAL)
    val quality: StateFlow<StreamQuality> = _quality.asStateFlow()
    private val _wifiOnly = MutableStateFlow(true)
    val wifiOnly: StateFlow<Boolean> = _wifiOnly.asStateFlow()

    /** Songs set aside (the server refused them) until pinned again; the worker, the UI and refresh touch it. */
    private val failed: MutableSet<String> = ConcurrentHashMap.newKeySet()

    private fun pinsFile() = File(app.filesDir, "downloads/pins.json")
    private fun prefs() = app.getSharedPreferences("downloads", Context.MODE_PRIVATE)

    @Synchronized
    fun init(context: Context) {
        if (started) return
        started = true
        app = context.applicationContext
        _quality.value = StreamQuality.fromId(prefs().getString("quality", null), StreamQuality.ORIGINAL)
        _wifiOnly.value = prefs().getBoolean("wifi_only", true)
        _pins.value = PinSet.decode(pinsFile().takeIf { it.isFile }?.readText())
        CoverStore.init(app)
        scope.launch {
            reconcile()
            if (_pins.value.wanted().any { it !in _stored.value.complete }) schedule()
            // Covers missing from the store (pinned before it existed, or fetched while offline): a
            // one-off burst now if the network is up, else the download worker's next run.
            CoverStore.sync(_pins.value, fetch = true)
        }
    }

    // ---- Settings ----------------------------------------------------------------------------

    fun setQuality(q: StreamQuality) {
        _quality.value = q
        prefs().edit().putString("quality", q.id).apply()
    }

    /** Wi-Fi only changes the worker's constraint: the waiting work is replaced. */
    fun setWifiOnly(on: Boolean) {
        _wifiOnly.value = on
        prefs().edit().putBoolean("wifi_only", on).apply()
        if (missing().isNotEmpty()) schedule(replace = true)
    }

    // ---- Queries -------------------------------------------------------------------------------

    fun isDownloaded(songId: String): Boolean = songId in _stored.value.complete

    fun isPinned(kind: PinKind, id: String): Boolean = _pins.value.isPinned(kind, id)

    /** A pinned album as an album page can show it, with no network. */
    fun pinnedAlbum(id: String): Album? = _pins.value[Pin.keyOf(PinKind.Album, id)]?.let(::albumOf)

    fun pinnedPlaylist(id: String): Playlist? = _pins.value[Pin.keyOf(PinKind.Playlist, id)]?.let(::playlistOf)

    fun albumOf(p: Pin) = Album(
        id = p.id, name = p.name, artist = p.artist, artistId = p.artistId, coverArt = p.coverArt, year = p.year,
        songCount = p.songs.size, duration = p.songs.sumOf { it.duration ?: 0 }, song = p.songs,
    )

    fun playlistOf(p: Pin) = Playlist(
        id = p.id, name = p.name, coverArt = p.coverArt, changed = p.changed,
        songCount = p.songs.size, duration = p.songs.sumOf { it.duration ?: 0 }, entry = p.songs,
    )

    /** Songs wanted and not complete (and not given up on), in download order. */
    fun missing(): List<String> {
        val s = _stored.value
        return _pins.value.wanted().filter { it !in s.complete && it !in failed }
    }

    fun progressOf(pin: Pin, stored: Stored, activity: Activity, net: NetState, wifiOnly: Boolean): PinProgress {
        val ids = pin.songIds.distinct()
        val done = ids.count { it in stored.complete }
        val failedHere = ids.count { it in stored.failed && it !in stored.complete }
        val bytes = ids.sumOf { stored.bytes[it] ?: 0L }
        var fraction = if (ids.isEmpty()) 1f else done.toFloat() / ids.size
        val current = activity.songId
        if (activity.running && current != null && current in ids && current !in stored.complete && activity.length > 0) {
            fraction += (activity.bytes.toFloat() / activity.length).coerceIn(0f, 1f) / ids.size
        }
        val state = when {
            done + failedHere >= ids.size -> PinProgress.State.Done
            activity.running && current in ids -> PinProgress.State.Downloading
            !net.online -> PinProgress.State.WaitingForNetwork
            wifiOnly && !net.unmetered -> PinProgress.State.WaitingForWifi
            activity.running -> PinProgress.State.Queued
            else -> PinProgress.State.Downloading // about to start
        }
        return PinProgress(ids.size, done, bytes, fraction.coerceIn(0f, 1f), state, failedHere)
    }

    // ---- Pinning -------------------------------------------------------------------------------

    /** Pins an item: albums and playlists are read from the server (or the saved answer) first. */
    suspend fun pin(item: Item): Boolean {
        val now = System.currentTimeMillis()
        val pin = try {
            when (item) {
                is Item.OfAlbum -> {
                    val a = Library.album(item.album.id)
                    Pin(PinKind.Album, a.id, a.name, a.artist, a.artistId, a.coverArt, a.year, a.song.map { it.forQueue() }, now, now)
                }
                is Item.OfPlaylist -> {
                    val p = runCatching { Library.fetchPlaylist(item.playlist.id) }.getOrElse { Library.playlist(item.playlist.id) }
                    Pin(PinKind.Playlist, p.id, p.name, coverArt = p.coverArt, songs = p.entry.map { it.forQueue() }, pinnedAtMs = now, syncedAtMs = now, changed = p.changed)
                }
                is Item.OfSong -> {
                    val s = item.song
                    Pin(PinKind.Song, s.id, s.title, s.artist, s.artistId, s.coverArt, s.year, listOf(s.forQueue()), now, now)
                }
                is Item.OfArtist -> return false
            }
        } catch (e: SubsonicError) {
            Library.post(errorMessage(e))
            return false
        }
        if (pin.songs.isEmpty()) {
            Library.post("Nothing to download")
            return false
        }
        val change = mutate { it.with(pin) }
        pin.songIds.forEach { failed.remove(it) }
        AppLog.i(TAG, "pinned ${pin.key}: ${pin.songs.size} songs, ${change.fetch.size} new to fetch")
        refresh()
        schedule()
        if (pin.kind == PinKind.Playlist) schedulePlaylistSync()
        // The covers come at once (a few hundred KB), so the item has its art offline even before
        // its songs are down; they wait for no Wi-Fi.
        scope.launch { CoverStore.sync(_pins.value, fetch = true); refresh() }
        Library.post(
            when {
                !NetworkMonitor.online -> "${pin.name} will download when you're back online"
                _wifiOnly.value && !NetworkMonitor.unmetered -> "${pin.name} will download on Wi-Fi"
                else -> "Downloading ${pin.name}"
            },
        )
        return true
    }

    suspend fun unpin(key: String) {
        val name = _pins.value[key]?.name
        val change = mutate { it.without(key) }
        deleteSongs(change.release)
        CoverStore.sync(_pins.value, fetch = false)
        AppLog.i(TAG, "removed $key: ${change.release.size} songs deleted")
        afterRemoval()
        name?.let { Library.post("Removed the download of $it") }
    }

    suspend fun removeAll() {
        val change = mutate { it.cleared() }
        deleteSongs(change.release)
        withContext(Dispatchers.IO) {
            // Anything left over (an interrupted write) goes too.
            val d = MediaStores.downloads(app)
            MediaStores.songIds(d).forEach { MediaStores.removeSong(d, it) }
        }
        CoverStore.clear()
        AppLog.i(TAG, "removed every download: ${change.release.size} songs")
        afterRemoval()
    }

    private suspend fun afterRemoval() {
        refresh()
        if (missing().isEmpty()) WorkManager.getInstance(app).cancelUniqueWork(WORK)
        if (_pins.value.pins.none { it.kind == PinKind.Playlist }) WorkManager.getInstance(app).cancelUniqueWork(SYNC_WORK)
    }

    // ---- Playlists follow the server -------------------------------------------------------------

    /** A playlist as the server just gave it: if it is pinned and changed, the pin follows. */
    suspend fun onPlaylistFetched(p: Playlist) {
        val pin = _pins.value[Pin.keyOf(PinKind.Playlist, p.id)] ?: return
        val diff = ListDiff.of(pin.songIds, p.entry.map { it.id })
        if (diff.isEmpty && pin.name == p.name) return
        val change = mutate { it.followPlaylist(p.id, p.entry.map { s -> s.forQueue() }, p.changed, System.currentTimeMillis()) }
        _pins.value[pin.key]?.let { renamed -> if (renamed.name != p.name) mutate { s -> s.with(renamed.copy(name = p.name, coverArt = p.coverArt)) } }
        deleteSongs(change.release)
        CoverStore.sync(_pins.value, fetch = true)
        AppLog.i(TAG, "playlist ${p.id} changed on the server: +${diff.added.size} -${diff.removed.size}; ${change.fetch.size} to fetch, ${change.release.size} deleted")
        refresh()
        if (change.fetch.isNotEmpty()) schedule()
    }

    /** Every pinned playlist, fetched and followed (app start, the network back, the daily job). */
    suspend fun syncPlaylists(): Int {
        val lists = _pins.value.pins.filter { it.kind == PinKind.Playlist }
        var synced = 0
        for (pin in lists) {
            try {
                Library.fetchPlaylist(pin.id) // follows the pin (onPlaylistFetched)
                synced++
            } catch (e: SubsonicError) {
                AppLog.w(TAG, "playlist ${pin.id}: not synced (${e.javaClass.simpleName})")
                if (e is SubsonicError.Network) break
            }
        }
        if (lists.isNotEmpty()) AppLog.i(TAG, "playlists synced: $synced of ${lists.size}")
        return synced
    }

    // ---- The worker's side ---------------------------------------------------------------------

    fun setActivity(a: Activity) {
        _activity.value = a
    }

    fun markFailed(songId: String) {
        failed += songId
        _stored.value = _stored.value.copy(failed = HashSet(failed))
    }

    /**
     * The quality to fetch a song in: one already started is finished in its quality. Otherwise the
     * download quality, or the original for an Opus file that needs no transcode
     * ([StreamQuality.forSource]; the pin's copy of the song says what the file is).
     */
    fun downloadQualityFor(songId: String): StreamQuality {
        val d = MediaStores.downloads(app)
        return StreamQuality.CHOICES.firstOrNull { d.getCachedBytes(CacheKeys.song(songId, it), 0, Long.MAX_VALUE) > 0 }
            ?: _quality.value.forSource(pinnedSong(songId)?.let(SourceFile::of))
    }

    private fun pinnedSong(songId: String): Song? =
        _pins.value.pins.firstNotNullOfOrNull { p -> p.songs.firstOrNull { it.id == songId } }

    fun isWanted(songId: String): Boolean = _pins.value.refCount(songId) > 0

    fun pinsWith(songId: String): List<Pin> = _pins.value.pins.filter { p -> p.songs.any { it.id == songId } }

    // ---- Plumbing ----------------------------------------------------------------------------

    private suspend fun mutate(block: (PinSet) -> PinChange): PinChange = mutex.withLock {
        val change = block(_pins.value)
        _pins.value = change.pins
        withContext(Dispatchers.IO) {
            val f = pinsFile()
            f.parentFile?.mkdirs()
            val tmp = File(f.parentFile, f.name + ".tmp")
            tmp.writeText(change.pins.encode())
            if (!tmp.renameTo(f)) {
                f.delete()
                tmp.renameTo(f)
            }
        }
        change
    }

    private suspend fun deleteSongs(ids: Set<String>) {
        if (ids.isEmpty()) return
        withContext(Dispatchers.IO) {
            val d = MediaStores.downloads(app)
            ids.forEach { MediaStores.removeSong(d, it) }
        }
    }

    /** Leftovers (not wanted) are deleted; then [stored] is read from the folder. */
    suspend fun reconcile() = withContext(Dispatchers.IO) {
        val d = MediaStores.downloads(app)
        val r = Reconcile.of(_pins.value.wanted(), MediaStores.songIds(d), emptySet())
        if (r.delete.isNotEmpty()) {
            r.delete.forEach { MediaStores.removeSong(d, it) }
            AppLog.i(TAG, "deleted ${r.delete.size} songs nothing pins")
        }
        refresh()
    }

    /** Reads what the downloads folder holds (in memory, cheap) into [stored]. */
    suspend fun refresh() = withContext(Dispatchers.IO) {
        val d = MediaStores.downloads(app)
        val ids = MediaStores.songIds(d)
        val complete = ids.filterTo(HashSet()) { MediaStores.completeQuality(d, it) != null }
        val bytes = ids.associateWith { MediaStores.bytesOf(d, it) }
        _stored.value = Stored(complete, bytes, d.cacheSpace + CoverStore.bytes(), HashSet(failed))
    }

    /** Starts (or queues behind the running one) the download work, with the network it needs. */
    fun schedule(replace: Boolean = false) {
        val network = if (_wifiOnly.value) NetworkType.UNMETERED else NetworkType.CONNECTED
        val request = OneTimeWorkRequestBuilder<DownloadWorker>()
            .setConstraints(Constraints.Builder().setRequiredNetworkType(network).setRequiresStorageNotLow(true).build())
            .setBackoffCriteria(androidx.work.BackoffPolicy.EXPONENTIAL, 30, TimeUnit.SECONDS)
            .build()
        val policy = if (replace) ExistingWorkPolicy.REPLACE else ExistingWorkPolicy.APPEND_OR_REPLACE
        WorkManager.getInstance(app).enqueueUniqueWork(WORK, policy, request)
        AppLog.i(TAG, "download work enqueued (${if (_wifiOnly.value) "unmetered" else "any"} network, ${missing().size} songs missing)")
    }

    /** The daily check of pinned playlists: on an unmetered network while charging. */
    private fun schedulePlaylistSync() {
        val request = PeriodicWorkRequestBuilder<PlaylistSyncWorker>(1, TimeUnit.DAYS)
            .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.UNMETERED).setRequiresCharging(true).build())
            .build()
        WorkManager.getInstance(app).enqueueUniquePeriodicWork(SYNC_WORK, ExistingPeriodicWorkPolicy.KEEP, request)
    }

    /** Sign out: every pin and download goes. */
    suspend fun clear() {
        WorkManager.getInstance(app).cancelUniqueWork(WORK)
        WorkManager.getInstance(app).cancelUniqueWork(SYNC_WORK)
        failed.clear()
        removeAll()
    }
}
