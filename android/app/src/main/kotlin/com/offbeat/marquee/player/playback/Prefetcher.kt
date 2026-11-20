package com.offbeat.marquee.player.playback

import com.offbeat.marquee.player.diag.AppLog
import androidx.annotation.OptIn
import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.cache.CacheWriter
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.runInterruptible
import kotlinx.coroutines.withContext
import java.io.IOException

/**
 * Fetches the play window (the current track and the next N, in play order) whole into the cache,
 * one track at a time, then stops, so the radio can sleep until the next track starts.
 *
 * [update] is called with the window whenever it may have changed (track change, queue edit,
 * shuffle or repeat change, play). A different window cancels the running fetch (it is stale) and
 * starts again; what is already cached is skipped at once. [onFetching] says whether anything in
 * the window is still missing, which decides the player's wake mode. A downloaded track counts as
 * stored and is never fetched.
 */
@OptIn(UnstableApi::class)
class Prefetcher(
    private val source: SongSource,
    private val scope: CoroutineScope,
    private val onFetching: (Boolean) -> Unit,
    private val onNetworkUsed: () -> Unit = {},
    /** A track of the window is now whole in the cache (a seek may have been waiting for it). */
    private val onCached: (songId: String) -> Unit = {},
) {
    private var window: List<String> = emptyList()
    private var job: Job? = null
    /** The last run ended with something in the window still missing (a fetch failed). */
    private var incomplete = false
    /** The network changed during a run: once it ends incomplete, it runs again (once). */
    private var retryAfterRun = false

    fun update(newWindow: List<String>) {
        if (newWindow == window && job != null) return
        window = newWindow
        job?.cancel()
        job = scope.launch { run(newWindow) }
    }

    /**
     * The network is back: a window left incomplete by failed fetches is fetched again, at once and
     * whole (still one burst), but only when something is playing.
     */
    fun retry() {
        if (window.isEmpty()) return
        if (job?.isActive == true) {
            retryAfterRun = true
            return
        }
        if (!incomplete) return
        AppLog.i(TAG, "network back: fetching the window again")
        val w = window
        job = scope.launch { run(w) }
    }

    /**
     * The same window, looked at again (Settings changed the quality, or the cache was cleared): what
     * is now missing is fetched, in one burst as usual.
     */
    fun restart(why: String) {
        if (window.isEmpty()) return
        AppLog.i(TAG, "$why: looking at the window again")
        job?.cancel()
        val w = window
        job = scope.launch { run(w) }
    }

    fun stop() {
        job?.cancel()
        job = null
        window = emptyList()
    }

    private suspend fun run(ids: List<String>) {
        val missing = withContext(Dispatchers.IO) { ids.filterNot { source.isStored(source.keyFor(it)) } }
        if (missing.isEmpty()) {
            AppLog.i(TAG, "window ${ids.size} tracks: all cached, nothing to fetch")
            incomplete = false
            onFetching(false)
            return
        }
        onFetching(true)
        var failed = false
        for (id in missing) {
            try {
                fetch(id)
                onNetworkUsed()
                onCached(id)
            } catch (e: CancellationException) {
                AppLog.i(TAG, "song $id: cancelled (window changed)")
                throw e
            } catch (e: IOException) {
                currentCoroutineContext().ensureActive() // a cancelled CacheWriter throws InterruptedIOException
                // Out of coverage, server error, read-only refusal: the player streams what it
                // can, and the next window change tries again.
                AppLog.w(TAG, "song $id: fetch failed (${e.javaClass.simpleName})")
                failed = true
            }
        }
        val allCached = !failed && withContext(Dispatchers.IO) { ids.all { source.isStored(source.keyFor(it)) } }
        AppLog.i(TAG, "window ${ids.size} tracks: done, ${if (allCached) "all cached, network idle" else "not all cached"}")
        incomplete = !allCached
        onFetching(!allCached)
        if (retryAfterRun && !allCached) {
            retryAfterRun = false
            AppLog.i(TAG, "network changed during the run: fetching the window again")
            run(ids)
        } else {
            retryAfterRun = false
        }
    }

    private suspend fun fetch(id: String) = coroutineScope {
        val spec = withContext(Dispatchers.IO) { source.dataSpecFor(id) }
        val key = spec.key!!
        val writer = CacheWriter(source.writerFactory.createDataSourceForDownloading(), spec, null, null)
        val canceller = launch { try { awaitCancellation() } finally { writer.cancel() } }
        val started = System.nanoTime()
        AppLog.i(TAG, "song $id: fetching (${key.substringAfterLast('/')})")
        try {
            runInterruptible(Dispatchers.IO) { writer.cache() }
        } finally {
            canceller.cancel()
        }
        val ms = (System.nanoTime() - started) / 1_000_000
        AppLog.i(TAG, "song $id: cached ${source.cachedBytes(key)} bytes in $ms ms")
    }

    private companion object {
        const val TAG = "MarqueePrefetch"
    }
}
