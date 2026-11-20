package com.offbeat.marquee.player.playback

import android.content.Context
import com.offbeat.marquee.core.playback.SavedQueue
import java.io.File

/** The queue, index and position in the app's private files; replaced atomically on each save. */
class QueueStore(context: Context) {
    private val file = File(context.applicationContext.filesDir, "queue.json")

    @Synchronized
    fun load(): SavedQueue? = if (file.isFile) runCatching { SavedQueue.decode(file.readText()) }.getOrNull() else null

    @Synchronized
    fun save(queue: SavedQueue) {
        val tmp = File(file.parentFile, "${file.name}.tmp")
        tmp.writeText(queue.encode())
        if (!tmp.renameTo(file)) {
            file.delete()
            tmp.renameTo(file)
        }
    }

    @Synchronized
    fun clear() {
        file.delete()
    }
}
