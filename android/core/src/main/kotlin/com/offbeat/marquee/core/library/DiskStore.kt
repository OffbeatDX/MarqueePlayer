package com.offbeat.marquee.core.library

import java.io.File
import java.io.IOException

/**
 * A small on-disk store of text by key (one file each), least recently used first out once there
 * are more than [maxEntries]. The app keeps its last good answers here so it opens instantly:
 * Home's shelves, and the album and artist pages opened before. Writes are atomic (temp file, then
 * rename), so a crash leaves the old answer or the new one, never half of one.
 */
class DiskStore(private val dir: File, private val maxEntries: Int) {
    init {
        require(maxEntries > 0)
    }

    @Synchronized
    fun read(key: String): String? {
        val f = fileFor(key)
        if (!f.isFile) return null
        return try {
            val text = f.readText()
            f.setLastModified(System.currentTimeMillis())
            text
        } catch (e: IOException) {
            null
        }
    }

    @Synchronized
    fun write(key: String, text: String) {
        try {
            dir.mkdirs()
            val f = fileFor(key)
            SafeFile.write(f, text)
            f.setLastModified(System.currentTimeMillis())
            trim()
        } catch (e: IOException) {
            // A cache: losing a write only costs a slower start.
        }
    }

    @Synchronized
    fun remove(key: String) {
        fileFor(key).delete()
    }

    @Synchronized
    fun clear() {
        dir.listFiles()?.forEach { it.delete() }
    }

    private fun entries(): List<File> = dir.listFiles { f -> f.isFile && f.name.endsWith(EXT) }?.toList().orEmpty()

    private fun trim() {
        val files = entries()
        if (files.size <= maxEntries) return
        files.sortedBy { it.lastModified() }.take(files.size - maxEntries).forEach { it.delete() }
    }

    private fun fileFor(key: String): File = File(dir, safeName(key) + EXT)

    companion object {
        private const val EXT = ".json"

        /** Letters, digits, '-', '_' and '.' are kept; anything else (the odd ':' in an id) becomes '_'. */
        fun safeName(key: String): String =
            key.map { if (it.isLetterOrDigit() || it == '-' || it == '_' || it == '.') it else '_' }.joinToString("").take(120)
    }
}
