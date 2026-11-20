package com.offbeat.marquee.player.offline

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.SystemClock
import com.offbeat.marquee.player.diag.AppLog
import androidx.annotation.OptIn
import androidx.core.app.NotificationCompat
import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.DataSpec
import androidx.media3.datasource.HttpDataSource
import androidx.media3.datasource.cache.CacheDataSource
import androidx.media3.datasource.cache.CacheWriter
import androidx.work.CoroutineWorker
import androidx.work.ForegroundInfo
import androidx.work.WorkerParameters
import com.offbeat.marquee.core.offline.Pin
import com.offbeat.marquee.core.playback.CacheKeys
import com.offbeat.marquee.core.playback.streamUrl
import com.offbeat.marquee.player.MainActivity
import com.offbeat.marquee.player.net.NetworkMonitor
import com.offbeat.marquee.player.playback.MediaStores
import com.offbeat.marquee.player.playback.StreamHttp
import com.offbeat.marquee.player.playback.ErrorReplyException
import com.offbeat.marquee.player.playback.Resumable
import androidx.media3.datasource.cache.SimpleCache
import kotlinx.coroutines.NonCancellable
import com.offbeat.marquee.core.playback.PlaybackRecovery
import com.offbeat.marquee.player.session.AppSession
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.launch
import kotlinx.coroutines.runInterruptible
import kotlinx.coroutines.withContext
import java.io.IOException

/**
 * Fetches the pinned songs that are missing, one at a time, into the downloads folder. Bytes the
 * cache already has are copied from it; the rest come from the server (the download quality, through
 * the session's client). A song interrupted half way resumes from where it stopped next time.
 *
 * WorkManager runs it only with the network it needs (unmetered while "Wi-Fi only" is on) and
 * stops it when that goes: it then returns retry and runs again once the network is back. A song
 * the server refuses (4xx) is set aside so the rest can go. While it runs it shows a progress
 * notification, in the foreground when the system allows it (it does not when the worker starts
 * with the app in the background: the notification is then an ordinary one).
 */
@OptIn(UnstableApi::class)
class DownloadWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    private val notifications = context.getSystemService(NotificationManager::class.java)
    private var lastNotified = 0L

    override suspend fun doWork(): Result {
        val ctx = applicationContext
        val client = withContext(Dispatchers.IO) { AppSession.ensureLoaded(ctx) } ?: return Result.failure()
        Downloads.init(ctx)
        Downloads.reconcile()
        CoverStore.sync(Downloads.pins.value, fetch = true)
        if (Downloads.missing().isEmpty()) return Result.success()
        ensureChannel(ctx)
        runCatching { setForeground(foregroundInfo(progressNotification(null))) }
            .onFailure { AppLog.i(TAG, "not in the foreground (${it.javaClass.simpleName}): an ordinary notification") }
        val writerFactory = CacheDataSource.Factory()
            .setCache(MediaStores.downloads(ctx))
            .setUpstreamDataSourceFactory(StreamHttp.cacheThenNetwork(ctx))
        var fetched = 0
        try {
            while (true) {
                val id = Downloads.missing().firstOrNull() ?: break
                val q = Downloads.downloadQualityFor(id)
                val spec = DataSpec.Builder().setUri(client.streamUrl(id, q)).setKey(CacheKeys.song(id, q)).build()
                val store = MediaStores.downloads(ctx)
                Downloads.setActivity(Activity(running = true, songId = id))
                notifyProgress(force = true)
                val started = SystemClock.elapsedRealtime()
                try {
                    try {
                        fetch(writerFactory, store, spec, id)
                    } catch (e: IOException) {
                        if (isStopped || !Resumable.isStale(e)) throw e
                        // Its first part is of a file the server has since replaced (or cut shorter): from the start, once.
                        AppLog.i(TAG, "song $id: the file changed on the server since its first part was downloaded: fetched again")
                        withContext(Dispatchers.IO) { Resumable.drop(store, spec.key!!) }
                        fetch(writerFactory, store, spec, id)
                    }
                } catch (e: CancellationException) {
                    throw e
                } catch (e: IOException) {
                    if (isStopped) throw CancellationException("stopped")
                    val reply = ErrorReplyException.findIn(e)
                    if (reply != null) {
                        // A Subsonic error in place of the song: nothing of it was stored.
                        if (reply.kind == PlaybackRecovery.FailureKind.Auth) {
                            AppLog.w(TAG, "song $id: the server refused the sign-in (code ${reply.code}); downloads stop until the next start")
                            return Result.failure()
                        }
                        AppLog.w(TAG, "song $id: the server answered an error (code ${reply.code ?: "none"}), set aside")
                        Downloads.markFailed(id)
                        continue
                    }
                    val status = (e as? HttpDataSource.InvalidResponseCodeException)?.responseCode
                    if (status != null && status in 400..499) {
                        AppLog.w(TAG, "song $id: the server refused it (HTTP $status), set aside")
                        Downloads.markFailed(id)
                        continue
                    }
                    AppLog.w(TAG, "song $id: interrupted (${e.javaClass.simpleName}), will resume; $fetched fetched this run")
                    return Result.retry()
                }
                fetched++
                AppLog.i(TAG, "song $id: downloaded (${q.id}, ${MediaStores.bytesOf(MediaStores.downloads(ctx), id)} bytes in ${SystemClock.elapsedRealtime() - started} ms)")
                // Unpinned while it was on its way: it goes again.
                if (!Downloads.isWanted(id)) withContext(Dispatchers.IO) { MediaStores.removeSong(MediaStores.downloads(ctx), id) }
                Downloads.refresh()
                if (Downloads.isWanted(id) && !Downloads.isDownloaded(id)) {
                    // Written to the end but its length never became known: don't loop on it.
                    AppLog.w(TAG, "song $id: written but not complete, set aside")
                    Downloads.markFailed(id)
                }
            }
            AppLog.i(TAG, "all pinned songs downloaded ($fetched this run)")
            if (fetched > 0) notifyDone(fetched)
            return Result.success()
        } finally {
            Downloads.setActivity(Activity())
            Downloads.refresh()
            notifications.cancel(NOTIFICATION_ID)
        }
    }

    /**
     * Fetches [plain] into the downloads ([store]). A part already there is completed only from the
     * same file ([Resumable]); the cache's copy is read for what it has only when it is whole (a part
     * of it could be of another version of the file than the download's part).
     */
    private suspend fun fetch(factory: CacheDataSource.Factory, store: SimpleCache, plain: DataSpec, id: String) = coroutineScope {
        val key = plain.key!!
        val spec = withContext(Dispatchers.IO) {
            val lru = MediaStores.cache(applicationContext)
            val lruComplete = MediaStores.isComplete(lru, key)
            if (!lruComplete && lru.getCachedBytes(key, 0, Long.MAX_VALUE) > 0) Resumable.drop(lru, key)
            if (lruComplete && store.getCachedBytes(key, 0, Long.MAX_VALUE) > 0) {
                // The cache has it whole: copied from there, consistent, instead of completing the part.
                Resumable.drop(store, key)
            }
            Resumable.prepare(store, plain, "song $id")
        }
        var remembered = false
        val writer = CacheWriter(factory.createDataSourceForDownloading(), spec, null) { length, cached, _ ->
            if (!remembered) {
                remembered = true
                Resumable.remember(store, spec)
            }
            Downloads.setActivity(Activity(running = true, songId = id, bytes = cached, length = length))
            notifyProgress(force = false)
        }
        val canceller = launch { try { awaitCancellation() } finally { writer.cancel() } }
        try {
            runInterruptible(Dispatchers.IO) { writer.cache() }
        } finally {
            canceller.cancel()
            withContext(NonCancellable + Dispatchers.IO) { Resumable.remember(store, spec) }
        }
    }

    // ---- Notification ----------------------------------------------------------------------------

    private fun notifyProgress(force: Boolean) {
        val now = SystemClock.elapsedRealtime()
        if (!force && now - lastNotified < 1_000) return
        lastNotified = now
        val a = Downloads.activity.value
        val pin = a.songId?.let { Downloads.pinsWith(it).firstOrNull() }
        runCatching { notifications.notify(NOTIFICATION_ID, progressNotification(pin)) }
    }

    private fun progressNotification(pin: Pin?): android.app.Notification {
        val ctx = applicationContext
        val missing = Downloads.missing().size
        val b = NotificationCompat.Builder(ctx, CHANNEL)
            .setSmallIcon(android.R.drawable.stat_sys_download)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setSilent(true)
            .setCategory(NotificationCompat.CATEGORY_PROGRESS)
            .setContentIntent(openApp(ctx))
            .setContentTitle(pin?.let { "Downloading ${it.name}" } ?: "Downloading")
        if (pin != null) {
            val p = Downloads.progressOf(pin, Downloads.stored.value, Downloads.activity.value, NetworkMonitor.state.value, false)
            b.setContentText("${p.done} of ${p.total} songs" + if (missing > 0) " · $missing to go in all" else "")
            b.setProgress(1000, (p.fraction * 1000).toInt(), false)
        } else {
            b.setContentText(if (missing == 1) "1 song" else "$missing songs").setProgress(0, 0, true)
        }
        return b.build()
    }

    private fun notifyDone(count: Int) {
        val ctx = applicationContext
        val n = NotificationCompat.Builder(ctx, CHANNEL)
            .setSmallIcon(android.R.drawable.stat_sys_download_done)
            .setContentTitle("Downloads finished")
            .setContentText(if (count == 1) "1 song downloaded" else "$count songs downloaded")
            .setSilent(true)
            .setAutoCancel(true)
            .setTimeoutAfter(15_000)
            .setContentIntent(openApp(ctx))
            .build()
        runCatching { notifications.notify(DONE_NOTIFICATION_ID, n) }
    }

    private fun foregroundInfo(n: android.app.Notification) =
        ForegroundInfo(NOTIFICATION_ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)

    override suspend fun getForegroundInfo(): ForegroundInfo {
        ensureChannel(applicationContext)
        return foregroundInfo(progressNotification(null))
    }

    companion object {
        private const val TAG = "MarqueeDownloadWork"
        const val CHANNEL = "downloads"
        const val NOTIFICATION_ID = 4101
        const val DONE_NOTIFICATION_ID = 4102

        fun ensureChannel(ctx: Context) {
            val nm = ctx.getSystemService(NotificationManager::class.java)
            if (nm.getNotificationChannel(CHANNEL) != null) return
            nm.createNotificationChannel(
                NotificationChannel(CHANNEL, "Downloads", NotificationManager.IMPORTANCE_LOW).apply {
                    description = "Progress of albums and playlists being downloaded for offline"
                },
            )
        }

        private fun openApp(ctx: Context): PendingIntent = PendingIntent.getActivity(
            ctx, 1, Intent(ctx, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
    }
}

/** The daily check of pinned playlists (WorkManager: unmetered network, charging). */
class PlaylistSyncWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        withContext(Dispatchers.IO) { AppSession.ensureLoaded(applicationContext) } ?: return Result.success()
        Downloads.init(applicationContext)
        Downloads.syncPlaylists()
        return Result.success()
    }
}
