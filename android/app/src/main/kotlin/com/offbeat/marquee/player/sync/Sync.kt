package com.offbeat.marquee.player.sync

import android.content.Context
import android.net.ConnectivityManager
import com.offbeat.marquee.player.diag.AppLog
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import com.offbeat.marquee.core.subsonic.SubsonicClient
import com.offbeat.marquee.core.subsonic.SubsonicError
import com.offbeat.marquee.core.sync.FlushResult
import com.offbeat.marquee.core.sync.Outbox
import com.offbeat.marquee.core.sync.OutboxSender
import com.offbeat.marquee.core.sync.TextFile
import com.offbeat.marquee.player.net.NetworkMonitor
import com.offbeat.marquee.player.session.AppSession
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.util.concurrent.TimeUnit

/**
 * The outbox ([Outbox]) for the whole process: scrobble submissions and favourite changes that
 * wait for the network, kept in `outbox.json`. Sent in batches, never by waking the radio for one:
 *
 * - when the network is up for something else: the radio became active ([ConnectivityManager.
 *   addDefaultNetworkActiveListener]), a prefetch just used it, or the network came back;
 * - at the latest a few hours after the first entry, by a WorkManager job with a CONNECTED
 *   constraint ([OutboxWorker]), cancelled once the outbox is empty.
 *
 * In read-only builds the client refuses every write locally: the entries are then dropped with a
 * log line, so they don't pile up.
 */
object Sync {
    private const val TAG = "MarqueeSync"
    const val WORK = "outbox"
    /** The latest a queued entry waits for the network to be up anyway. */
    const val LATEST_HOURS = 2L

    private lateinit var app: Context
    lateinit var outbox: Outbox
        private set
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var started = false

    @Synchronized
    fun init(context: Context) {
        if (started) return
        started = true
        app = context.applicationContext
        val file = File(app.filesDir, "outbox.json")
        outbox = Outbox(
            file = object : TextFile {
                override fun read(): String? = if (file.isFile) runCatching { file.readText() }.getOrNull() else null
                override fun write(text: String) {
                    val tmp = File(file.parentFile, file.name + ".tmp")
                    tmp.writeText(text)
                    if (!tmp.renameTo(file)) {
                        file.delete()
                        tmp.renameTo(file)
                    }
                }
            },
            clock = System::currentTimeMillis,
            log = { AppLog.i(TAG, it) },
        )
        if (!outbox.isEmpty) AppLog.i(TAG, "outbox: ${outbox.size} waiting from before")
        scope.launch { NetworkMonitor.cameBack.collect { flush(force = true, why = "network back") } }
        app.getSystemService(ConnectivityManager::class.java)
            .addDefaultNetworkActiveListener { flushIfDue("radio active") }
    }

    fun queueScrobble(songId: String, playedAtMs: Long) {
        outbox.addScrobble(songId, playedAtMs)
        AppLog.i(TAG, "outbox: scrobble $songId queued, ${outbox.size} waiting")
        afterQueue()
    }

    fun queueStar(kind: String, id: String, starred: Boolean) {
        outbox.setStar(kind, id, starred, System.currentTimeMillis())
        AppLog.i(TAG, "outbox: favourite $kind $id=$starred queued, ${outbox.size} waiting")
        afterQueue()
    }

    private fun afterQueue() {
        schedule()
        val active = runCatching { app.getSystemService(ConnectivityManager::class.java).isDefaultNetworkActive }.getOrDefault(false)
        if (active) flushIfDue("radio already active")
    }

    /** Something else is using the network: send what is waiting, unless backing off. */
    fun flushIfDue(why: String) {
        if (!started || !NetworkMonitor.online || !outbox.isDue()) return
        scope.launch { flush(force = false, why = why) }
    }

    suspend fun flush(force: Boolean, why: String, cancelJob: Boolean = true): FlushResult {
        if (outbox.isEmpty) return FlushResult.Empty
        val client = withContext(Dispatchers.IO) { AppSession.ensureLoaded(app) } ?: return FlushResult.Empty
        AppLog.i(TAG, "outbox: flushing ${outbox.size} ($why)")
        val result = outbox.flush(senderFor(client), force)
        if (cancelJob && outbox.isEmpty) WorkManager.getInstance(app).cancelUniqueWork(WORK)
        return result
    }

    /** The safety net: the latest the outbox waits, on any network. The first entry sets the time. */
    private fun schedule() {
        val request = OneTimeWorkRequestBuilder<OutboxWorker>()
            .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
            .setInitialDelay(LATEST_HOURS, TimeUnit.HOURS)
            .setBackoffCriteria(androidx.work.BackoffPolicy.EXPONENTIAL, 15, TimeUnit.MINUTES)
            .build()
        WorkManager.getInstance(app).enqueueUniqueWork(WORK, ExistingWorkPolicy.KEEP, request)
    }

    private fun senderFor(c: SubsonicClient) = object : OutboxSender {
        override suspend fun scrobble(ids: List<String>, timesMs: List<Long>) = c.scrobble(ids, timesMs)

        override suspend fun star(kind: String, id: String, starred: Boolean) {
            val (song, album, artist) = when (kind) {
                "album" -> Triple(null, id, null)
                "artist" -> Triple(null, null, id)
                else -> Triple(id, null, null)
            }
            if (starred) c.star(song, album, artist) else c.unstar(song, album, artist)
        }
    }

    /** Sign out. */
    fun clear() {
        if (!started) return
        outbox.clear()
        WorkManager.getInstance(app).cancelUniqueWork(WORK)
    }
}

class OutboxWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        Sync.init(applicationContext)
        return when (Sync.flush(force = true, why = "scheduled job", cancelJob = false)) {
            is FlushResult.Retry -> Result.retry()
            else -> Result.success()
        }
    }
}

/** Whether an error means "the network", for deciding to queue rather than give up. */
fun SubsonicError.isNetwork(): Boolean = this is SubsonicError.Network
