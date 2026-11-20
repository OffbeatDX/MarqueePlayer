package com.offbeat.marquee.player.playback

import android.content.Context
import com.offbeat.marquee.player.diag.AppLog
import androidx.annotation.OptIn
import androidx.media3.common.util.UnstableApi
import androidx.media3.database.StandaloneDatabaseProvider
import androidx.media3.datasource.cache.CacheDataSource
import androidx.media3.datasource.cache.ContentMetadata
import androidx.media3.datasource.cache.NoOpCacheEvictor
import androidx.media3.datasource.cache.SimpleCache
import androidx.media3.datasource.okhttp.OkHttpDataSource
import com.offbeat.marquee.core.playback.CacheKeys
import com.offbeat.marquee.core.playback.StreamQuality
import com.offbeat.marquee.core.subsonic.SubsonicClient
import com.offbeat.marquee.player.session.AppSession
import okhttp3.Call
import okhttp3.OkHttpClient
import java.io.File
import java.io.IOException
import java.lang.ref.WeakReference
import java.util.concurrent.ConcurrentLinkedQueue
import java.util.concurrent.TimeUnit

/**
 * The two places song bytes are kept, one SimpleCache each, per process:
 *
 * - the **cache** (app cache folder, LRU, capped: 2 GB by default): prefetched and recently played
 *   tracks, and the notification's covers. The system may reclaim it; "Clear cache" empties it.
 * - the **downloads** (app files folder, never evicted): what the user pinned. Only the download
 *   worker writes it and only removing a pin deletes from it.
 *
 * Both use the same keys (`song/<id>/<quality>`). The player reads downloads first, then the
 * cache, then the network ([SongSource]).
 */
@OptIn(UnstableApi::class)
object MediaStores {
    private const val TAG = "MarqueeStores"
    @Volatile private var database: StandaloneDatabaseProvider? = null
    @Volatile private var lru: SimpleCache? = null
    @Volatile private var downloads: SimpleCache? = null

    private fun database(context: Context): StandaloneDatabaseProvider = database ?: synchronized(this) {
        database ?: StandaloneDatabaseProvider(context.applicationContext).also { database = it }
    }

    @Volatile private var evictor: CappedLruEvictor? = null

    /** The LRU cache, capped by Settings > Cache ([setCap] changes the cap while it is open). */
    fun cache(context: Context): SimpleCache = lru ?: synchronized(this) {
        lru ?: run {
            PlaybackSettings.init(context)
            val e = CappedLruEvictor(PlaybackSettings.cacheCapBytes)
            evictor = e
            SimpleCache(cacheDir(context), e, database(context))
        }.also { lru = it }
    }

    /** A new cap for the cache: lowering it evicts the least recently used tracks now. Blocking (disk). */
    fun setCap(context: Context, bytes: Long) {
        val c = cache(context)
        evictor?.setCap(c, bytes)
    }

    fun downloads(context: Context): SimpleCache = downloads ?: synchronized(this) {
        downloads ?: SimpleCache(downloadsDir(context), NoOpCacheEvictor(), database(context)).also { downloads = it }
    }

    fun cacheDir(context: Context): File = File(context.applicationContext.cacheDir, "media")
    fun downloadsDir(context: Context): File = File(context.applicationContext.filesDir, "downloads/media")

    fun isComplete(cache: SimpleCache, key: String): Boolean {
        val length = ContentMetadata.getContentLength(cache.getContentMetadata(key))
        return length > 0 && cache.isCached(key, 0, length)
    }

    /** The quality a song is complete in, in [cache], or null. */
    fun completeQuality(cache: SimpleCache, songId: String): StreamQuality? =
        StreamQuality.CHOICES.firstOrNull { isComplete(cache, CacheKeys.song(songId, it)) }

    /** Bytes stored for a song in any quality. */
    fun bytesOf(cache: SimpleCache, songId: String): Long =
        StreamQuality.CHOICES.sumOf { cache.getCachedBytes(CacheKeys.song(songId, it), 0, Long.MAX_VALUE).coerceAtLeast(0) }

    /** The songs [cache] holds anything of (complete or not). */
    fun songIds(cache: SimpleCache): Set<String> = cache.keys.mapNotNullTo(HashSet()) { CacheKeys.songId(it) }

    fun removeSong(cache: SimpleCache, songId: String) {
        for (q in StreamQuality.CHOICES) cache.removeResource(CacheKeys.song(songId, q))
    }

    /** Empties the LRU cache (Settings, "Clear cache"). Downloads are not touched. */
    fun clearCache(context: Context): Long {
        val c = cache(context)
        val before = c.cacheSpace
        for (key in c.keys.toList()) c.removeResource(key)
        AppLog.i(TAG, "cache cleared: ${before - c.cacheSpace} bytes freed, ${c.cacheSpace} left (in use)")
        return before - c.cacheSpace
    }
}

/**
 * The HTTP side of streaming and downloading, through the session's OkHttp client (one connection
 * pool, the same-origin redirect policy, and the read-only guard in test builds) with a shorter
 * read timeout than the UI's: a stream
 * that sends nothing for [READ_TIMEOUT_S] seconds is dead (a connection left over from a network
 * that went away), and the player or downloader retries on a fresh one.
 *
 * Every call made here is remembered (weakly, the last few) so that [dropConnections] can cancel
 * the ones still reading when the network changes: a socket bound to a network that is gone can
 * block a read until the timeout, with the bytes the player waits for already in the cache.
 */
@OptIn(UnstableApi::class)
object StreamHttp {
    private const val TAG = "MarqueeStreamHttp"
    const val READ_TIMEOUT_S = 12L
    private const val REMEMBERED = 16

    /** Debug comparisons only (see PlaybackService.stallFixes): false streams with the session's own timeouts. */
    @Volatile var shortReadTimeout = true

    private var base: OkHttpClient? = null
    private var derived: OkHttpClient? = null
    private val calls = ConcurrentLinkedQueue<WeakReference<Call>>()

    @Synchronized
    internal fun clientFor(c: SubsonicClient): OkHttpClient {
        if (base !== c.httpClient || derived == null) {
            base = c.httpClient
            derived = c.httpClient.newBuilder().readTimeout(READ_TIMEOUT_S, TimeUnit.SECONDS).build()
        }
        return derived!!
    }

    val callFactory = Call.Factory { request ->
        // An IOException, as SongSource.dataSpecFor has it: the player and the prefetcher take it for
        // an interrupted read (a sign-out while a track is fetched), not a crash.
        val client = AppSession.current ?: throw IOException("Not signed in")
        val call = (if (shortReadTimeout) clientFor(client) else client.httpClient).newCall(request)
        calls.add(WeakReference(call))
        while (calls.size > REMEMBERED) calls.poll()
        call
    }

    val upstream: OkHttpDataSource.Factory = OkHttpDataSource.Factory(callFactory)

    private val worker = java.util.concurrent.Executors.newSingleThreadExecutor { r -> Thread(r, "marquee-drop-connections") }

    /**
     * Cancels the streams still open and closes idle connections, so the next request opens a fresh
     * one. Off the main thread: closing a TLS socket writes to it.
     */
    fun dropConnections(why: String) {
        worker.execute { dropNow(why) }
    }

    private fun dropNow(why: String) {
        var cancelled = 0
        for (ref in calls) {
            val call = ref.get() ?: continue
            if (!call.isCanceled()) {
                call.cancel()
                cancelled++
            }
        }
        calls.clear()
        AppSession.current?.httpClient?.connectionPool?.evictAll()
        AppLog.i(TAG, "dropped connections ($why): $cancelled calls cancelled, idle connections closed")
    }

    /** Reads the LRU cache, the network for what is missing; never writes (the player's and the downloader's upstream). */
    fun cacheThenNetwork(context: Context): CacheDataSource.Factory = CacheDataSource.Factory()
        .setCache(MediaStores.cache(context))
        .setUpstreamDataSourceFactory(upstream)
        .setCacheWriteDataSinkFactory(null)
        .setFlags(CacheDataSource.FLAG_IGNORE_CACHE_ON_ERROR)
}
