package com.offbeat.marquee.core.sync

import com.offbeat.marquee.core.subsonic.SubsonicError
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

/** Exponential backoff: [initialMs] after the first failure, times [factor] after each one, at most [maxMs]. */
class Backoff(val initialMs: Long, val maxMs: Long, val factor: Double = 2.0) {
    init {
        require(initialMs > 0 && maxMs >= initialMs && factor >= 1.0)
    }

    /** The wait after the [failures]th failure in a row (1 = the first). */
    fun delayFor(failures: Int): Long {
        if (failures <= 0) return 0
        var d = initialMs.toDouble()
        repeat(failures - 1) {
            d *= factor
            if (d >= maxMs) return maxMs
        }
        return d.toLong().coerceAtMost(maxMs)
    }

    companion object {
        /** Scrobbles and favourites waiting to be sent: a minute, then doubling, at most six hours. */
        val OUTBOX = Backoff(60_000, 6 * 3_600_000L)

        /** A track that failed to load: 2 s, 4 s, 8 s ... at most 30 s. */
        val PLAYBACK = Backoff(2_000, 30_000)
    }
}

/** A play to submit, with when it started (the `time` parameter). */
@Serializable
data class QueuedScrobble(val id: String, val timeMs: Long)

/** A favourite change made while it could not be sent. [kind] is "song", "album" or "artist". */
@Serializable
data class QueuedStar(val kind: String, val id: String, val starred: Boolean, val atMs: Long)

@Serializable
data class OutboxData(
    val scrobbles: List<QueuedScrobble> = emptyList(),
    val stars: List<QueuedStar> = emptyList(),
    val failures: Int = 0,
    val nextAttemptAtMs: Long = 0,
)

/** Where the outbox lives between runs (a file in the app; memory in tests). */
interface TextFile {
    fun read(): String?
    fun write(text: String)
}

/** The writes the outbox makes. Implemented by the Subsonic client in the app, faked in tests. */
interface OutboxSender {
    /** Several plays in one call (Subsonic takes `id` and `time` repeated). */
    suspend fun scrobble(ids: List<String>, timesMs: List<Long>)
    suspend fun star(kind: String, id: String, starred: Boolean)
}

sealed interface FlushResult {
    data object Empty : FlushResult
    data object Done : FlushResult
    /** Backing off from a failure: nothing is tried before [atMs]. */
    data class NotDue(val atMs: Long) : FlushResult
    /** Failed (network); try again at [atMs]. */
    data class Retry(val atMs: Long) : FlushResult
}

/**
 * Writes that wait for the network, kept on disk: scrobble submissions (with their play time) and
 * favourite changes made offline. [flush] sends them in order, scrobbles in batches; on a network
 * failure it stops and backs off ([Backoff.OUTBOX]), keeping everything; what the server or a
 * read-only build refuses is dropped with a log line, so nothing piles up. A scrobble already
 * queued (same song, same time) is not queued again, and the queue keeps at most [maxScrobbles]
 * (the oldest go first).
 */
class Outbox(
    private val file: TextFile,
    private val clock: () -> Long,
    private val backoff: Backoff = Backoff.OUTBOX,
    private val maxScrobbles: Int = 1000,
    private val batchSize: Int = 50,
    private val log: (String) -> Unit = {},
) {
    private val lock = Any()
    private val sending = Mutex()
    private var data: OutboxData = load()

    private fun load(): OutboxData = file.read()?.let { text ->
        runCatching { json.decodeFromString(OutboxData.serializer(), text) }.getOrNull()
    } ?: OutboxData()

    private fun save() {
        file.write(json.encodeToString(OutboxData.serializer(), data))
    }

    private inline fun edit(block: (OutboxData) -> OutboxData) = synchronized(lock) {
        data = block(data)
        save()
    }

    fun snapshot(): OutboxData = synchronized(lock) { data }

    val size: Int get() = synchronized(lock) { data.scrobbles.size + data.stars.size }
    val isEmpty: Boolean get() = size == 0

    /** Something to send and not backing off. */
    fun isDue(): Boolean = synchronized(lock) { (data.scrobbles.isNotEmpty() || data.stars.isNotEmpty()) && clock() >= data.nextAttemptAtMs }

    fun addScrobble(id: String, timeMs: Long) = edit { d ->
        val s = QueuedScrobble(id, timeMs)
        if (s in d.scrobbles) return@edit d
        val list = (d.scrobbles + s).sortedBy { it.timeMs }
        d.copy(scrobbles = if (list.size > maxScrobbles) list.takeLast(maxScrobbles) else list)
    }

    /** The last change to an item wins: a newer one replaces what was waiting. */
    fun setStar(kind: String, id: String, starred: Boolean, atMs: Long) = edit { d ->
        d.copy(stars = d.stars.filterNot { it.kind == kind && it.id == id } + QueuedStar(kind, id, starred, atMs))
    }

    /** Whether a favourite change for [id] is still waiting (its local state then stays as set). */
    fun pendingStar(id: String): QueuedStar? = synchronized(lock) { data.stars.lastOrNull { it.id == id } }

    fun clear() = edit { OutboxData() }

    /**
     * Sends what is queued, favourites first, then scrobbles oldest first in batches. [force]
     * ignores the backoff (the network just came back).
     */
    suspend fun flush(sender: OutboxSender, force: Boolean = false): FlushResult = sending.withLock {
        val first = snapshot()
        if (first.scrobbles.isEmpty() && first.stars.isEmpty()) return FlushResult.Empty
        if (!force && clock() < first.nextAttemptAtMs) return FlushResult.NotDue(first.nextAttemptAtMs)
        // Until empty: what is queued while sending goes out in the same flush.
        while (true) {
            val start = snapshot()
            if (start.scrobbles.isEmpty() && start.stars.isEmpty()) break
            for (star in start.stars) {
                val outcome = attempt("favourite ${star.kind} ${star.id}") { sender.star(star.kind, star.id, star.starred) }
                if (outcome is FlushResult.Retry) return outcome
                edit { d -> d.copy(stars = d.stars - star) }
            }
            for (batch in start.scrobbles.chunked(batchSize)) {
                val what = if (batch.size == 1) "scrobble ${batch[0].id}" else "${batch.size} scrobbles"
                val outcome = attempt(what) { sender.scrobble(batch.map { it.id }, batch.map { it.timeMs }) }
                if (outcome is FlushResult.Retry) return outcome
                val sent = batch.toSet()
                edit { d -> d.copy(scrobbles = d.scrobbles.filterNot { it in sent }) }
            }
        }
        edit { d -> d.copy(failures = 0, nextAttemptAtMs = 0) }
        FlushResult.Done
    }

    /** One send: null when it went (or was dropped for good), [FlushResult.Retry] to stop and back off. */
    private suspend fun attempt(what: String, send: suspend () -> Unit): FlushResult? {
        try {
            send()
            log("outbox: $what sent")
            return null
        } catch (e: SubsonicError.RefusedReadOnly) {
            log("outbox: $what dropped: ${e.endpoint} refused (read-only build), not sent")
            return null
        } catch (e: SubsonicError.Network) {
            return backOff(what, "network")
        } catch (e: SubsonicError.AuthFailed) {
            return backOff(what, "sign-in refused")
        } catch (e: SubsonicError) {
            // The server answered and said no (an unknown song, say): sending again won't help.
            log("outbox: $what dropped: the server refused it (${e.javaClass.simpleName})")
            return null
        }
    }

    private fun backOff(what: String, why: String): FlushResult.Retry {
        var at = 0L
        edit { d ->
            val failures = d.failures + 1
            at = clock() + backoff.delayFor(failures)
            d.copy(failures = failures, nextAttemptAtMs = at)
        }
        log("outbox: $what not sent ($why), ${size} waiting, next try in ${(at - clock()) / 1000} s")
        return FlushResult.Retry(at)
    }

    private companion object {
        val json = Json { ignoreUnknownKeys = true; encodeDefaults = false }
    }
}
