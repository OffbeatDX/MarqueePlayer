package com.offbeat.marquee.player.playback

import android.content.Context
import com.offbeat.marquee.core.playback.SavedQueue
import java.io.File
import java.util.concurrent.atomic.AtomicLong

/**
 * The queue, index and position in the app's private files; replaced atomically on each save.
 * Each snapshot or clear carries a sequence number from [nextSeq], so an older save finishing late
 * on an IO thread never overwrites a newer one (such as the final save in `onDestroy`).
 */
class QueueStore(private val file: File) {
    constructor(context: Context) : this(File(context.applicationContext.filesDir, "queue.json"))

    private val seqGen = AtomicLong(0)
    private var lastSeq = 0L

    /** Issues the next monotonically increasing sequence number for a queue snapshot or clear. */
    fun nextSeq(): Long = seqGen.incrementAndGet()

    @Synchronized
    fun load(): SavedQueue? = if (file.isFile) runCatching { SavedQueue.decode(file.readText()) }.getOrNull() else null

    /**
     * Writes [queue] atomically when [seq] is newer than the last write or clear; skips older
     * snapshots arriving out of order. Returns true when the file was updated.
     */
    @Synchronized
    fun save(queue: SavedQueue, seq: Long = nextSeq()): Boolean {
        if (seq <= lastSeq) return false
        lastSeq = seq
        file.parentFile?.mkdirs()
        val tmp = File(file.parentFile, "${file.name}.tmp")
        tmp.writeText(queue.encode())
        if (!tmp.renameTo(file)) {
            file.delete()
            if (!tmp.renameTo(file)) tmp.delete()
        }
        return true
    }

    /**
     * Removes the saved queue when [seq] is newer than the last write or clear; skips older clears
     * arriving out of order. Returns true when the file was cleared.
     */
    @Synchronized
    fun clear(seq: Long = nextSeq()): Boolean {
        if (seq <= lastSeq) return false
        lastSeq = seq
        file.delete()
        return true
    }
}
