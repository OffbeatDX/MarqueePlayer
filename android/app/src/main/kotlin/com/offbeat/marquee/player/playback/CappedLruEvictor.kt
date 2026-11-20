package com.offbeat.marquee.player.playback

import com.offbeat.marquee.player.diag.AppLog
import androidx.annotation.OptIn
import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.cache.Cache
import androidx.media3.datasource.cache.CacheEvictor
import androidx.media3.datasource.cache.CacheSpan
import com.offbeat.marquee.core.settings.CacheCap
import java.util.TreeSet

/**
 * Media3's least-recently-used evictor with a cap that can change while the cache is open
 * (Settings > Cache): [setCap] lowers it and trims at once, oldest spans first ([CacheCap.evictions]).
 * All calls come with the cache's lock held, as for Media3's own evictor; [trim] takes it.
 */
@OptIn(UnstableApi::class)
class CappedLruEvictor(@Volatile private var maxBytes: Long) : CacheEvictor {
    private val spans = TreeSet<CacheSpan> { a, b ->
        val t = a.lastTouchTimestamp.compareTo(b.lastTouchTimestamp)
        if (t != 0) t else a.compareTo(b)
    }
    private var used = 0L

    override fun requiresCacheSpanTouches(): Boolean = true
    override fun onCacheInitialized() {}

    override fun onStartFile(cache: Cache, key: String, position: Long, length: Long) {
        if (length != androidx.media3.common.C.LENGTH_UNSET.toLong()) evict(cache, length)
    }

    override fun onSpanAdded(cache: Cache, span: CacheSpan) {
        spans.add(span)
        used += span.length
        evict(cache, 0)
    }

    override fun onSpanRemoved(cache: Cache, span: CacheSpan) {
        spans.remove(span)
        used -= span.length
    }

    override fun onSpanTouched(cache: Cache, oldSpan: CacheSpan, newSpan: CacheSpan) {
        onSpanRemoved(cache, oldSpan)
        onSpanAdded(cache, newSpan)
    }

    /** The new cap; the cache is trimmed to it now (a lower cap) or simply has more room (a higher one). */
    fun setCap(cache: Cache, bytes: Long) {
        if (bytes == maxBytes) return
        val before = cache.cacheSpace
        maxBytes = bytes
        synchronized(cache) { evict(cache, 0) }
        AppLog.i(TAG, "cache cap ${bytes / MB} MB: ${before / MB} MB before, ${cache.cacheSpace / MB} MB after")
    }

    private fun evict(cache: Cache, incoming: Long) {
        if (used + incoming <= maxBytes) return
        val oldest = spans.toList()
        val n = CacheCap.evictions(oldest.map { it.length }, used, maxBytes, incoming)
        for (span in oldest.take(n)) cache.removeSpan(span)
    }

    private companion object {
        const val TAG = "MarqueeStores"
        const val MB = 1024L * 1024
    }
}
