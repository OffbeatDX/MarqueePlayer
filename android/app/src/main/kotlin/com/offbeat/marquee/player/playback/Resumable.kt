package com.offbeat.marquee.player.playback

import androidx.annotation.OptIn
import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.DataSpec
import androidx.media3.datasource.HttpDataSource
import androidx.media3.datasource.cache.ContentMetadataMutations
import androidx.media3.datasource.cache.SimpleCache
import com.offbeat.marquee.core.playback.Resume
import com.offbeat.marquee.player.diag.AppLog
import java.io.IOException

/**
 * Fetching a song into a store ([MediaStores]) so that a part already there is only completed from
 * the same file ([Resume]): the validator of every answer is kept in the store's metadata for the key,
 * a resumed fetch sends it as `If-Range`, and [isStale] tells the caller to drop the part and fetch
 * the song again from the start (once).
 */
@OptIn(UnstableApi::class)
object Resumable {
    private const val TAG = "MarqueeResume"
    private const val META_VALIDATOR = "marquee-validator"

    fun validatorOf(store: SimpleCache, key: String): String? =
        store.getContentMetadata(key).get(META_VALIDATOR, null as String?)

    /** The spec to fetch with: [spec] as it is, or with `If-Range` when a checked part is stored; an unchecked part is dropped. */
    fun prepare(store: SimpleCache, spec: DataSpec, what: String): DataSpec {
        val key = spec.key ?: return spec
        val stored = store.getCachedBytes(key, 0, Long.MAX_VALUE).coerceAtLeast(0)
        if (MediaStores.isComplete(store, key)) return spec
        return when (val plan = Resume.plan(stored, validatorOf(store, key))) {
            Resume.Plan.Start -> spec
            Resume.Plan.Restart -> {
                AppLog.i(TAG, "$what: $stored bytes stored with nothing to check them by: fetched again from the start")
                drop(store, key)
                spec
            }
            is Resume.Plan.Continue -> spec.withAdditionalHeaders(mapOf("If-Range" to plan.ifRange))
        }
    }

    /** Keeps the validator the server gave for [spec] with its bytes in [store] (as soon as bytes come, and at the end). */
    fun remember(store: SimpleCache, spec: DataSpec) {
        val key = spec.key ?: return
        val v = StreamHttp.validatorSeen(spec.uri.toString()) ?: return
        if (validatorOf(store, key) == v) return
        runCatching {
            store.applyContentMetadataMutations(key, ContentMetadataMutations().set(META_VALIDATOR, v))
        }
    }

    fun drop(store: SimpleCache, key: String) {
        store.removeResource(key)
    }

    /**
     * The fetch failed because the part stored is not of the file the server has now: it answered
     * the `If-Range` with the whole new file ([FileChangedException]), or refused the range (416, a
     * shorter file). The part is to be dropped and the song fetched again from the start.
     */
    fun isStale(e: Throwable): Boolean {
        var cur: Throwable? = e
        val seen = HashSet<Throwable>()
        while (cur != null && seen.add(cur)) {
            if (cur is FileChangedException) return true
            if (cur is HttpDataSource.InvalidResponseCodeException && cur.responseCode == 416) return true
            cur = cur.cause
        }
        return false
    }
}

/** A resumed fetch found the file changed on the server since its first part was stored. */
class FileChangedException : IOException("The file changed on the server since the part stored")
