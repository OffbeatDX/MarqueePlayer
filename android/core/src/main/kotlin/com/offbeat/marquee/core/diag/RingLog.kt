package com.offbeat.marquee.core.diag

import java.io.File
import java.io.FileOutputStream
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter

/**
 * The app's log on the phone: two files of at most [maxBytes] / 2 each in [dir], `app.log` being
 * written and `app.1.log` the one before it. When `app.log` would grow past its half it becomes
 * `app.1.log` (the older one goes), so the log never holds more than [maxBytes] and always keeps
 * the latest half at least. Thread-safe; callers keep it off the main thread.
 */
class RingLog(private val dir: File, private val maxBytes: Long = DEFAULT_MAX_BYTES) {
    val current = File(dir, "app.log")
    val previous = File(dir, "app.1.log")
    private val half = maxBytes / 2

    @Synchronized
    fun append(text: String) {
        if (text.isEmpty()) return
        var bytes = text.toByteArray(Charsets.UTF_8)
        if (bytes.size > half) bytes = bytes.copyOfRange(bytes.size - half.toInt(), bytes.size)
        if (!dir.isDirectory) dir.mkdirs()
        if (current.length() + bytes.size > half) rotate()
        FileOutputStream(current, true).use { it.write(bytes) }
    }

    /** Both files, oldest first. */
    @Synchronized
    fun read(): String = buildString {
        if (previous.isFile) append(previous.readText(Charsets.UTF_8))
        if (current.isFile) append(current.readText(Charsets.UTF_8))
    }

    @Synchronized
    fun sizeBytes(): Long = previous.length() + current.length()

    @Synchronized
    fun clear() {
        previous.delete()
        current.delete()
    }

    private fun rotate() {
        previous.delete()
        if (current.isFile && !current.renameTo(previous)) current.delete()
    }

    companion object {
        const val DEFAULT_MAX_BYTES = 500L * 1024
    }
}

/** One log line as the file keeps it: `2026-11-17 10:23:45.123 W Tag: message`, then the trace. */
object LogLine {
    private val TIME = DateTimeFormatter.ofPattern("yyyy-MM-dd HH:mm:ss.SSS")
    const val MAX_MESSAGE = 2000

    fun format(
        timeMs: Long,
        level: Char,
        tag: String,
        message: String,
        trace: String? = null,
        zone: ZoneId = ZoneId.systemDefault(),
    ): String = buildString {
        append(TIME.format(Instant.ofEpochMilli(timeMs).atZone(zone)))
        append(' ').append(level).append(' ').append(tag).append(": ")
        // One entry per line: a message with line breaks keeps them indented.
        val m = if (message.length > MAX_MESSAGE) message.take(MAX_MESSAGE) + "… (${message.length} chars)" else message
        append(m.replace("\n", "\n    "))
        append('\n')
        if (!trace.isNullOrBlank()) {
            for (line in trace.trimEnd().lines()) append("    ").append(line).append('\n')
        }
    }

    fun time(timeMs: Long, zone: ZoneId = ZoneId.systemDefault()): String = TIME.format(Instant.ofEpochMilli(timeMs).atZone(zone))
}
