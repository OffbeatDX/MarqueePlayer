package com.offbeat.marquee.player.diag

import android.content.Context
import android.util.Log
import com.offbeat.marquee.core.diag.CrashHandler
import com.offbeat.marquee.core.diag.LogLine
import com.offbeat.marquee.core.diag.RingLog
import com.offbeat.marquee.core.diag.Scrubber
import com.offbeat.marquee.core.subsonic.SubsonicCredentials
import com.offbeat.marquee.player.session.AppSession
import java.io.File
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit

/**
 * The app's one logger: every line goes to logcat as before, and info, warnings and errors also go
 * to a small log kept on the phone (`files/logs`, [RingLog], about 500 KB), which "Share
 * diagnostics" attaches. Debug and verbose lines stay in logcat only.
 *
 * File writes happen on one background thread (it ends after 30 s with nothing to write), and each
 * line is scrubbed there first ([Scrubber]: no URLs, token, salt, user name or server host). That
 * thread's first job is loading the stored session, so its values are known before any line is
 * written. Nothing is ever sent anywhere from here.
 */
object AppLog {
    private const val LAST_CRASH = "last-crash.txt"

    private var ring: RingLog? = null
    private var dir: File? = null
    @Volatile private var scrubber: Scrubber = Scrubber.NONE
    @Volatile private var dropped = 0

    private val writer = ThreadPoolExecutor(0, 1, 30, TimeUnit.SECONDS, LinkedBlockingQueue(2_000)) { r ->
        Thread(r, "marquee-log").apply { isDaemon = true; priority = Thread.MIN_PRIORITY }
    }

    /** First thing in Application.onCreate: the crash handler, the log directory, the session's values. */
    fun init(context: Context) {
        val app = context.applicationContext
        val d = File(app.filesDir, "logs")
        dir = d
        ring = RingLog(d)
        CrashHandler.install(::recordCrash)
        submit { runCatching { AppSession.ensureLoaded(app) } }
    }

    fun v(tag: String, msg: String) = Log.v(tag, msg)
    fun d(tag: String, msg: String) = Log.d(tag, msg)

    fun i(tag: String, msg: String) {
        Log.i(tag, msg)
        enqueue('I', tag, msg, null)
    }

    fun w(tag: String, msg: String, tr: Throwable? = null) {
        if (tr != null) Log.w(tag, msg, tr) else Log.w(tag, msg)
        enqueue('W', tag, msg, tr)
    }

    fun e(tag: String, msg: String, tr: Throwable? = null) {
        if (tr != null) Log.e(tag, msg, tr) else Log.e(tag, msg)
        enqueue('E', tag, msg, tr)
    }

    /** The signed-in account's values, hidden from every line written from now on. */
    fun hide(credentials: SubsonicCredentials) = hide(credentials.url, credentials.user, credentials.salt, credentials.token)

    /** Also the sign-in form's address and user, before the server has answered. */
    fun hide(url: String?, user: String?, salt: String? = null, token: String? = null) {
        val more = Scrubber.of(url, user, salt, token)
        synchronized(this) { scrubber += more }
    }

    /** The scrubber as it stands (every account this process has seen). */
    val currentScrubber: Scrubber get() = scrubber

    /** The log, oldest first, after what is waiting has been written. Blocking. */
    fun read(): String {
        flush()
        return ring?.read().orEmpty()
    }

    fun sizeBytes(): Long = ring?.sizeBytes() ?: 0L

    fun lastCrash(): String? = dir?.let { File(it, LAST_CRASH) }?.takeIf { it.isFile }?.readText()

    /** Sign out: the log and the last crash go with the session. Blocking. */
    fun clear() {
        flush()
        ring?.clear()
        dir?.let { File(it, LAST_CRASH).delete() }
        dropped = 0
    }

    private fun enqueue(level: Char, tag: String, msg: String, tr: Throwable?) {
        if (ring == null) return
        val time = System.currentTimeMillis()
        val trace = tr?.let(CrashHandler::stackTrace)
        submit { write(LogLine.format(time, level, tag, msg, trace)) }
    }

    private fun write(line: String) {
        val r = ring ?: return
        val lost = dropped
        if (lost > 0) {
            dropped = 0
            r.append(LogLine.format(System.currentTimeMillis(), 'W', "MarqueeLog", "$lost lines not kept (too many at once)"))
        }
        r.append(scrubber.scrub(line))
    }

    private fun submit(task: () -> Unit) {
        try {
            writer.execute { runCatching(task) }
        } catch (_: RejectedExecutionException) {
            dropped++
        }
    }

    /** Waits for what is queued (at most 2 s). */
    private fun flush() {
        val done = java.util.concurrent.CountDownLatch(1)
        submit { done.countDown() }
        done.await(2, TimeUnit.SECONDS)
    }

    /**
     * The uncaught-exception handler's part, on the crashing thread: what is still queued is
     * written here, then the crash, to the log and to `last-crash.txt`. Android's own handler then
     * ends the process as before.
     */
    private fun recordCrash(thread: Thread, error: Throwable) {
        val pending = ArrayList<Runnable>()
        writer.queue.drainTo(pending)
        for (r in pending) runCatching { r.run() }
        val now = System.currentTimeMillis()
        val text = CrashHandler.describe(now, thread, error, scrubber)
        ring?.append(LogLine.format(now, 'E', "MarqueeCrash", text))
        dir?.let { d ->
            d.mkdirs()
            File(d, LAST_CRASH).writeText(text)
        }
    }
}
