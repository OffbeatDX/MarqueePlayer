package com.offbeat.marquee.core.library

import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.nio.file.AtomicMoveNotSupportedException
import java.nio.file.Files
import java.nio.file.StandardCopyOption

/**
 * A small file replaced whole, so that a crash, a full disk or a power cut leaves the old content or
 * the new, never half of one and never nothing: written to `<name>.tmp`, synced to the disk, then
 * renamed over the file (one atomic rename on Android). If that rename fails, the temp file stays and
 * [read] takes it while the file itself is missing, so a write is never lost to a failed second rename.
 *
 * For the app's own state: the pins, the outbox, the favourites, the saved queue, the saved answers.
 */
object SafeFile {
    fun tmpOf(file: File): File = File(file.parentFile, file.name + ".tmp")
    fun badOf(file: File): File = File(file.parentFile, file.name + ".bad")

    /** Replaces [file] with [text]. Throws when it could not be written; the old content then stays. */
    fun write(file: File, text: String) {
        file.parentFile?.mkdirs()
        val tmp = tmpOf(file)
        FileOutputStream(tmp).use { out ->
            out.write(text.toByteArray(Charsets.UTF_8))
            out.flush()
            out.fd.sync()
        }
        try {
            Files.move(tmp.toPath(), file.toPath(), StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
        } catch (e: AtomicMoveNotSupportedException) {
            Files.move(tmp.toPath(), file.toPath(), StandardCopyOption.REPLACE_EXISTING)
        }
    }

    /**
     * The content, or null when there is none. The temp file is taken when the file is missing
     * (a write whose rename failed). Throws when a file is there but can't be read.
     */
    fun read(file: File): String? {
        if (file.isFile) return file.readText()
        val tmp = tmpOf(file)
        if (tmp.isFile) return tmp.readText()
        return null
    }

    /** [read], with null for a file that can't be read as well. */
    fun readOrNull(file: File): String? = try {
        read(file)
    } catch (e: IOException) {
        null
    }

    /**
     * Keeps a file that could not be understood as `<name>.bad` (replacing an older one), so a later
     * version or a person can still look at it; the next [write] starts afresh. Returns where it went.
     */
    fun setAside(file: File): File? {
        val src = if (file.isFile) file else tmpOf(file).takeIf { it.isFile } ?: return null
        val bad = badOf(file)
        return try {
            Files.move(src.toPath(), bad.toPath(), StandardCopyOption.REPLACE_EXISTING)
            bad
        } catch (e: IOException) {
            null
        }
    }

    /** The file and a leftover temp file go. */
    fun delete(file: File) {
        tmpOf(file).delete()
        file.delete()
    }
}
