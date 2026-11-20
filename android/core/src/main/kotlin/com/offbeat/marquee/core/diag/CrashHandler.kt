package com.offbeat.marquee.core.diag

import java.io.PrintWriter
import java.io.StringWriter

/**
 * The uncaught-exception handler: [record] writes the crash down (it must not throw, but a
 * failure there is swallowed), then the [previous] handler runs as it would have, so Android still
 * shows its dialog and ends the process.
 */
class CrashHandler(
    private val previous: Thread.UncaughtExceptionHandler?,
    private val record: (Thread, Throwable) -> Unit,
) : Thread.UncaughtExceptionHandler {

    override fun uncaughtException(thread: Thread, error: Throwable) {
        try {
            record(thread, error)
        } catch (_: Throwable) {
            // Nothing more can be done about a crash while writing a crash down.
        }
        previous?.uncaughtException(thread, error)
    }

    companion object {
        /** Installs one in front of the current default handler. */
        fun install(record: (Thread, Throwable) -> Unit): CrashHandler =
            CrashHandler(Thread.getDefaultUncaughtExceptionHandler(), record).also { Thread.setDefaultUncaughtExceptionHandler(it) }

        fun stackTrace(error: Throwable): String = StringWriter().also { error.printStackTrace(PrintWriter(it)) }.toString()

        /** What `last-crash.txt` holds: when, on which thread, and the scrubbed stack trace. */
        fun describe(timeMs: Long, thread: Thread, error: Throwable, scrubber: Scrubber): String =
            scrubber.scrub("Crashed at ${LogLine.time(timeMs)} on thread \"${thread.name}\"\n${stackTrace(error).trimEnd()}\n")
    }
}
