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
    /** Failed transiently (network, sign-in, or server trouble); try again at [atMs]. */
    data class Retry(val atMs: Long) : FlushResult
}

/**
 * Whether this failure may clear up by itself, so the write should wait in the outbox and be tried
 * again later: the network, a sign-in the server refused (the password may be fixed), an address
 * that answered with something other than Subsonic (a captive portal), an unusable address, or
 * server trouble without a Subsonic refusal (a bare HTTP error such as 5xx, 408 or 429, or Subsonic
 * code 0). What the server explicitly refuses (a non-zero Subsonic code) and what a read-only build
 * blocks are not transient: sending again would not help. The outbox and every direct write use this
 * one rule, so "queue it" and "the server said no" mean the same thing everywhere.
 */
fun SubsonicError.isTransient(): Boolean = when (this) {
    is SubsonicError.Network, is SubsonicError.AuthFailed, is SubsonicError.NotSubsonic, is SubsonicError.InvalidUrl -> true
    is SubsonicError.Server -> code == null || code == 0
    is SubsonicError.RefusedReadOnly -> false
}

/** How a write tried directly went: see [sendOrQueue]. */
sealed interface DirectWrite {
    data object Sent : DirectWrite
    /** Offline ([error] null) or a transient failure: it waits in the outbox. */
    data class Queued(val error: SubsonicError?) : DirectWrite
    /** The server refused it, or a read-only build blocked it: nothing was queued. */
    data class Refused(val error: SubsonicError) : DirectWrite
}

/**
 * Sends a write now when [online]; offline, or on a transient failure ([isTransient]), hands it to
 * [queue] instead, so it goes out later with the outbox.
 */
suspend fun sendOrQueue(online: Boolean, send: suspend () -> Unit, queue: () -> Unit): DirectWrite {
    if (!online) {
        queue()
        return DirectWrite.Queued(null)
    }
    return try {
        send()
        DirectWrite.Sent
    } catch (e: SubsonicError) {
        if (e.isTransient()) {
            queue()
            DirectWrite.Queued(e)
        } else {
            DirectWrite.Refused(e)
        }
    }
}

/**
 * Writes that wait for the network, kept on disk: scrobble submissions (with their play time) and
 * favourite changes made offline. [flush] sends them in order, scrobbles in batches; on a network,
 * sign-in, captive-portal or transient server error it stops and backs off ([Backoff.OUTBOX]),
 * keeping everything; what the server explicitly refuses (a non-zero Subsonic error code) or a
 * read-only build blocks is dropped with a log line, so nothing piles up. When a batch of several
 * scrobbles is refused permanently, its items are retried one by one so one bad song ID does not
 * drop the rest. A scrobble already queued (same song, same time) is not queued again, and the
 * queue keeps at most [maxScrobbles] (the oldest go first).
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
                if (outcome is Attempt.Retry) return outcome.result
                edit { d -> d.copy(stars = d.stars - star) }
            }
            for (batch in start.scrobbles.chunked(batchSize)) {
                val what = if (batch.size == 1) "scrobble ${batch[0].id}" else "${batch.size} scrobbles"
                when (val outcome = attempt(what, logRefusal = batch.size == 1) { sender.scrobble(batch.map { it.id }, batch.map { it.timeMs }) }) {
                    is Attempt.Retry -> return outcome.result
                    Attempt.Sent, Attempt.Dropped -> {
                        val sent = batch.toSet()
                        edit { d -> d.copy(scrobbles = d.scrobbles.filterNot { it in sent }) }
                    }
                    is Attempt.Refused -> {
                        if (batch.size == 1) {
                            edit { d -> d.copy(scrobbles = d.scrobbles - batch[0]) }
                        } else {
                            log("outbox: ${batch.size} scrobbles refused (code ${outcome.code}), retrying one by one")
                            for (item in batch) {
                                val single = attempt("scrobble ${item.id}") { sender.scrobble(listOf(item.id), listOf(item.timeMs)) }
                                if (single is Attempt.Retry) return single.result
                                edit { d -> d.copy(scrobbles = d.scrobbles - item) }
                            }
                        }
                    }
                }
            }
        }
        edit { d -> d.copy(failures = 0, nextAttemptAtMs = 0) }
        FlushResult.Done
    }

    private sealed interface Attempt {
        data object Sent : Attempt
        data object Dropped : Attempt
        data class Refused(val code: Int) : Attempt
        data class Retry(val result: FlushResult.Retry) : Attempt
    }

    /** One send: [Attempt.Sent] / [Attempt.Dropped] / [Attempt.Refused] to remove it, [Attempt.Retry] to stop and back off. */
    private suspend fun attempt(what: String, logRefusal: Boolean = true, send: suspend () -> Unit): Attempt {
        try {
            send()
            log("outbox: $what sent")
            return Attempt.Sent
        } catch (e: SubsonicError) {
            return when {
                e is SubsonicError.RefusedReadOnly -> {
                    log("outbox: $what dropped: ${e.endpoint} refused (read-only build), not sent")
                    Attempt.Dropped
                }
                e.isTransient() -> Attempt.Retry(backOff(what, retryReason(e)))
                else -> {
                    // Explicit Subsonic refusal (e.g. 70 not found, 10/20/30 version/parameter, 50 unauthorized): sending again won't help.
                    val code = (e as? SubsonicError.Server)?.code ?: -1
                    if (logRefusal) log("outbox: $what dropped: the server refused it (code $code)")
                    Attempt.Refused(code)
                }
            }
        }
    }

    private fun retryReason(e: SubsonicError): String = when (e) {
        is SubsonicError.Network -> "network"
        is SubsonicError.AuthFailed -> "sign-in refused"
        is SubsonicError.NotSubsonic -> "unexpected response"
        is SubsonicError.InvalidUrl -> "invalid address"
        is SubsonicError.Server -> when {
            e.code == 0 -> "server error 0"
            e.httpStatus != null -> "HTTP ${e.httpStatus}"
            else -> "server error"
        }
        is SubsonicError.RefusedReadOnly -> "read-only"
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
