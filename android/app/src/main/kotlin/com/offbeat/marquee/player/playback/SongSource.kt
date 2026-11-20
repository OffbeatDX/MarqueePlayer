package com.offbeat.marquee.player.playback

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.Uri
import com.offbeat.marquee.player.diag.AppLog
import androidx.annotation.OptIn
import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.DataSpec
import androidx.media3.datasource.ResolvingDataSource
import androidx.media3.datasource.cache.CacheDataSource
import androidx.media3.datasource.cache.SimpleCache
import com.offbeat.marquee.core.playback.CacheKeys
import com.offbeat.marquee.core.playback.NetworkKind
import com.offbeat.marquee.core.playback.SourceFile
import com.offbeat.marquee.core.playback.SourcePicker
import com.offbeat.marquee.core.playback.StreamQuality
import com.offbeat.marquee.core.playback.streamUrl
import com.offbeat.marquee.core.subsonic.Song
import com.offbeat.marquee.player.session.AppSession
import java.io.IOException
import java.util.concurrent.ConcurrentHashMap

/**
 * Where a song's bytes come from: the downloads first, then the cache, then the server (through
 * the session's OkHttp client, [StreamHttp]) for what is missing.
 *
 * Media items carry a placeholder URI, `marquee://song/<id>` (no token, safe to hand to other
 * apps). The real stream URL is built only when the player or the prefetcher opens the song, and
 * the quality is decided then, once per track ([SourcePicker]), and remembered while the track is
 * in the play window so a seek or a retry reads the same bytes.
 *
 * The decision needs the song's file (an Opus file may play as it is, [SourceFile]): the placeholder
 * URI carries its suffix, content type and bitrate, read when the player opens it, and the service
 * hands over the play window's songs for the prefetcher ([learn]).
 */
@OptIn(UnstableApi::class)
class SongSource(context: Context, private val settings: PlaybackSettings) {
    private val app = context.applicationContext
    private val connectivity = app.getSystemService(ConnectivityManager::class.java)
    val cache: SimpleCache = MediaStores.cache(app)
    val downloads: SimpleCache = MediaStores.downloads(app)

    private val decisions = ConcurrentHashMap<String, StreamQuality>()
    private val files = ConcurrentHashMap<String, SourceFile>()

    /**
     * The player reads the downloads and the cache but writes neither: the prefetcher writes whole
     * tracks into the cache, and a second writer would only take the span lock away from it. While
     * a span is being fetched the player reads that part from the network and moves over to the
     * cache as soon as the prefetcher has written past it (CacheDataSource re-checks every 100 KB).
     */
    val playerDataSourceFactory: ResolvingDataSource.Factory = ResolvingDataSource.Factory(
        CacheDataSource.Factory()
            .setCache(downloads)
            .setUpstreamDataSourceFactory(StreamHttp.cacheThenNetwork(app))
            .setCacheWriteDataSinkFactory(null)
            .setFlags(CacheDataSource.FLAG_IGNORE_CACHE_ON_ERROR),
    ) { spec -> resolve(spec) }

    /** For [androidx.media3.datasource.cache.CacheWriter]: reads the cache, fetches and writes the holes. */
    val writerFactory: CacheDataSource.Factory = CacheDataSource.Factory()
        .setCache(cache)
        .setUpstreamDataSourceFactory(StreamHttp.upstream)

    fun network(): NetworkKind {
        val net = connectivity.activeNetwork ?: return NetworkKind.None
        val caps = connectivity.getNetworkCapabilities(net) ?: return NetworkKind.None
        if (!caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)) return NetworkKind.None
        val unmetered = caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED) ||
            caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_TEMPORARILY_NOT_METERED)
        return if (unmetered) NetworkKind.Unmetered else NetworkKind.Metered
    }

    /** The quality this song plays in, decided on first use and then kept (see [retainOnly]). */
    fun qualityFor(songId: String, known: SourceFile? = null): StreamQuality = decisions.computeIfAbsent(songId) {
        val net = network()
        val file = known ?: files[songId]
        val q = SourcePicker.pick(songId, net, settings.quality, file, ::isDownloaded, ::isCachedFully)
        val key = CacheKeys.song(songId, q)
        val where = when {
            isDownloaded(key) -> "downloaded"
            isCachedFully(key) -> "fully cached"
            else -> "not fully stored"
        }
        val what = file?.let { "${it.suffix ?: "?"} ${it.bitRate ?: "?"} kbps" } ?: "file unknown"
        AppLog.i(TAG, "song $songId: ${q.id} (network ${net.name.lowercase()}, $where, $what)")
        q
    }

    /** Forget the decisions for songs that left the play window, so they are decided afresh. */
    fun retainOnly(songIds: Set<String>) {
        decisions.keys.retainAll(songIds)
        files.keys.retainAll(songIds)
    }

    /** What the song's file is, for its decision; learned before it is first decided. */
    fun learn(song: Song) {
        files[song.id] = SourceFile.of(song)
    }

    /** A decision made before a download finished or the network changed is made again. */
    fun forget(songId: String) {
        decisions.remove(songId)
    }

    /**
     * The network changed: songs other than [keep] (the current one and the next, which the player may
     * already be reading for gapless playback) with nothing stored yet are decided afresh
     * (on Wi-Fi they become the original: no transcode, and no waiting on a transcode the server is
     * still feeding to a request that was abandoned on the old network). Those keep their format: a decoder
     * may have started on it.
     */
    fun redecideUnfetched(keep: Set<String>, why: String = "network changed") {
        val gone = decisions.entries.filter { (id, q) -> id !in keep && cachedBytes(CacheKeys.song(id, q)) <= 0 && !isDownloaded(CacheKeys.song(id, q)) }.map { it.key }
        gone.forEach { decisions.remove(it) }
        if (gone.isNotEmpty()) AppLog.i(TAG, "$why: ${gone.size} upcoming songs will be decided again")
    }

    fun keyFor(songId: String): String = CacheKeys.song(songId, qualityFor(songId))

    fun isDownloaded(key: String): Boolean = MediaStores.isComplete(downloads, key)
    fun isCachedFully(key: String): Boolean = MediaStores.isComplete(cache, key)

    /** Complete on the phone (downloaded or whole in the cache), so it plays without a network. */
    fun isStored(key: String): Boolean = isDownloaded(key) || isCachedFully(key)

    /** Any copy of the song complete on the phone, whatever its quality. */
    fun isStoredAnyQuality(songId: String): Boolean =
        MediaStores.completeQuality(downloads, songId) != null || MediaStores.completeQuality(cache, songId) != null

    fun cachedBytes(key: String): Long = cache.getCachedBytes(key, 0, Long.MAX_VALUE)

    /** The real request for a song: its stream URL (with the token) and its cache key. */
    fun dataSpecFor(songId: String, base: DataSpec? = null, file: SourceFile? = null): DataSpec {
        val client = AppSession.current ?: throw IOException("Not signed in")
        val q = qualityFor(songId, file)
        val b = base?.buildUpon() ?: DataSpec.Builder()
        return b.setUri(client.streamUrl(songId, q)).setKey(CacheKeys.song(songId, q)).build()
    }

    private fun resolve(spec: DataSpec): DataSpec {
        val id = songIdOf(spec.uri) ?: return spec
        val file = sourceFileOf(spec.uri)?.also { files[id] = it }
        return dataSpecFor(id, spec, file)
    }

    companion object {
        private const val TAG = "MarqueeSource"
        const val SCHEME = "marquee"

        /** The placeholder for a song: its id, and what its file is (no token: safe to hand out). */
        fun songUri(song: Song): Uri = Uri.Builder().scheme(SCHEME).authority("song").appendPath(song.id).apply {
            song.suffix?.let { appendQueryParameter(Q_SUFFIX, it) }
            song.contentType?.let { appendQueryParameter(Q_TYPE, it) }
            song.bitRate?.let { appendQueryParameter(Q_KBPS, it.toString()) }
        }.build()

        /** The file a [songUri] describes; null for one made before it carried any. */
        fun sourceFileOf(uri: Uri): SourceFile? {
            val suffix = uri.getQueryParameter(Q_SUFFIX)
            val type = uri.getQueryParameter(Q_TYPE)
            val kbps = uri.getQueryParameter(Q_KBPS)?.toIntOrNull()
            return if (suffix == null && type == null && kbps == null) null else SourceFile(suffix, type, kbps)
        }

        private const val Q_SUFFIX = "suffix"
        private const val Q_TYPE = "type"
        private const val Q_KBPS = "kbps"

        fun songIdOf(uri: Uri): String? =
            if (uri.scheme == SCHEME && uri.authority == "song") uri.lastPathSegment else null
    }
}
