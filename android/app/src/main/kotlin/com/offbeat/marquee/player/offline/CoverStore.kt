package com.offbeat.marquee.player.offline

import android.content.Context
import com.offbeat.marquee.player.diag.AppLog
import com.offbeat.marquee.core.offline.CoverFile
import com.offbeat.marquee.core.offline.PinSet
import com.offbeat.marquee.core.offline.PinnedCovers
import com.offbeat.marquee.player.net.NetworkMonitor
import com.offbeat.marquee.player.session.AppSession
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import okhttp3.Request
import java.io.File
import java.io.IOException
import java.util.concurrent.ConcurrentHashMap

/**
 * The covers downloads keep ([PinnedCovers]): plain files in `files/downloads/covers/`, apart from
 * Coil's evictable cover cache (`cache/covers`), so clearing that cache, the system trimming it or
 * its LRU eviction never leaves a downloaded album on the placeholder offline. Removing the
 * download removes them.
 *
 * Everything that draws a cover asks [file] first ([com.offbeat.marquee.player.art.Images], the
 * cover provider behind the notification, Auto and the widget, and the accent sampler).
 */
object CoverStore {
    private const val TAG = "MarqueeCoverStore"

    private var dir: File? = null
    /** Cover id to the sizes stored, read from the folder once (a directory listing). */
    private val index = ConcurrentHashMap<String, Set<Int>>()
    private val mutex = Mutex()
    /** Covers the server would not give this run (no art): not asked again until the app restarts. */
    private val refused = ConcurrentHashMap.newKeySet<CoverFile>()

    @Synchronized
    fun init(context: Context) {
        if (dir != null) return
        val d = File(context.applicationContext.filesDir, "downloads/covers")
        dir = d
        d.listFiles()?.forEach { f -> PinnedCovers.parse(f.name)?.let { add(it) } ?: f.delete() }
    }

    /** The stored file that answers a cover at [size] ([PinnedCovers.pick]), or null. */
    fun file(coverId: String, size: Int, offline: Boolean = !NetworkMonitor.online): File? {
        val d = dir ?: return null
        val sizes = index[coverId] ?: return null
        val pick = PinnedCovers.pick(size, sizes, offline) ?: return null
        return File(d, PinnedCovers.fileName(CoverFile(coverId, pick))).takeIf { it.isFile }
    }

    fun stored(): Set<CoverFile> = index.flatMapTo(HashSet()) { (id, sizes) -> sizes.map { CoverFile(id, it) } }

    fun bytes(): Long = dir?.listFiles()?.sumOf { it.length() } ?: 0L

    /**
     * Makes the folder match [pins]: what nothing wants is deleted; with [fetch] (and the network up)
     * the missing covers are fetched through the session's client, one at a time, a few hundred KB
     * per album. Returns how many are still missing.
     */
    suspend fun sync(pins: PinSet, fetch: Boolean): Int = mutex.withLock {
        withContext(Dispatchers.IO) {
            val d = dir ?: return@withContext 0
            val plan = PinnedCovers.plan(PinnedCovers.wanted(pins), stored())
            for (f in plan.delete) {
                File(d, PinnedCovers.fileName(f)).delete()
                remove(f)
            }
            val todo = plan.fetch.filter { it !in refused }
            if (plan.delete.isNotEmpty()) AppLog.i(TAG, "deleted ${plan.delete.size} covers nothing pins")
            if (!fetch || todo.isEmpty() || !NetworkMonitor.online) return@withContext todo.size
            val client = AppSession.current ?: return@withContext todo.size
            d.mkdirs()
            var got = 0
            var bytes = 0L
            for ((i, f) in todo.withIndex()) {
                try {
                    bytes += download(client, f, d)
                    got++
                } catch (e: IOException) {
                    AppLog.w(TAG, "covers: stopped after $got of ${todo.size} (${e.javaClass.simpleName})")
                    return@withContext todo.size - i
                } catch (e: NotAnImage) {
                    refused += f
                }
            }
            AppLog.i(TAG, "covers: $got stored ($bytes bytes), ${todo.size - got} without art")
            0
        }
    }

    /** Every stored cover goes (remove all downloads, sign out). */
    suspend fun clear() = mutex.withLock {
        withContext(Dispatchers.IO) {
            dir?.deleteRecursively()
            index.clear()
            refused.clear()
        }
    }

    private class NotAnImage : Exception()

    private fun download(client: com.offbeat.marquee.core.subsonic.SubsonicClient, f: CoverFile, d: File): Long {
        val request = Request.Builder().url(client.coverArtUrl(f.coverId, f.size)).build()
        client.httpClient.newCall(request).execute().use { r ->
            // A missing cover is a Subsonic error (JSON) or a 404: nothing to keep.
            val type = r.header("Content-Type").orEmpty()
            if (r.code == 404 || (r.isSuccessful && !type.startsWith("image/"))) throw NotAnImage()
            if (!r.isSuccessful) throw IOException("HTTP ${r.code}")
            val body = r.body ?: throw IOException("no body")
            val target = File(d, PinnedCovers.fileName(f))
            val tmp = File(d, target.name + ".part")
            tmp.outputStream().use { out -> body.byteStream().copyTo(out) }
            if (!tmp.renameTo(target)) {
                target.delete()
                if (!tmp.renameTo(target)) throw IOException("rename failed")
            }
            add(f)
            return target.length()
        }
    }

    private fun add(f: CoverFile) {
        index.compute(f.coverId) { _, s -> (s ?: emptySet()) + f.size }
    }

    private fun remove(f: CoverFile) {
        index.computeIfPresent(f.coverId) { _, s -> (s - f.size).ifEmpty { null } }
    }
}
