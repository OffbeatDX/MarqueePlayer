package com.offbeat.marquee.core.lyrics

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/**
 * The order lyrics are shown in: the server's synced > LRCLIB's synced > the server's plain >
 * LRCLIB's plain > LRCLIB's "instrumental" > nothing. Synced lyrics are worth more than whose they
 * are; between two of the same kind the user's own server wins (its tags or `.lrc` files were put
 * there on purpose).
 */
object SourceOrder {
    fun pick(server: Lyrics?, lrclib: LyricsAnswer?): LyricsAnswer {
        val online = (lrclib as? LyricsAnswer.Found)?.lyrics
        return when {
            server != null && server.synced -> LyricsAnswer.Found(server)
            online != null && online.synced -> LyricsAnswer.Found(online)
            server != null -> LyricsAnswer.Found(server)
            online != null -> LyricsAnswer.Found(online)
            lrclib == LyricsAnswer.Instrumental -> LyricsAnswer.Instrumental
            else -> LyricsAnswer.NotFound
        }
    }
}

/** Why a lookup failed (nothing cached could stand in). */
class LyricsLookupError(val busy: Boolean, cause: Throwable) : Exception(cause.message, cause)

/**
 * Finds a song's lyrics: the cache first, then the server (`getLyricsBySongId`), then, when the
 * server has no synced lyrics and looking up online is on, LRCLIB. Each source is asked at most
 * once per call, and only when the cache does not already know its answer. With looking up online
 * off, LRCLIB's cached answers are kept but not shown. One lookup runs at a
 * time; after a 429/503 LRCLIB is left alone for its `Retry-After` (a minute when it says nothing).
 *
 * [server] returns the server's entries (empty when it has none) and throws on a network error;
 * any other failure (an old server without the endpoint) counts as "none". [lrclib] is null when
 * looking up online is off (always, in debug and benchmark builds).
 */
class LyricsResolver(
    private val cache: LyricsCache,
    private val server: suspend (songId: String) -> List<StructuredLyrics>,
    private val lrclib: () -> LrclibClient?,
    private val deviceLanguages: () -> List<String>,
    private val isNetworkError: (Throwable) -> Boolean = { false },
) {
    enum class Mode {
        /** Whatever the cache does not know yet. */
        Normal,

        /** The user pulled the view: ask the server again (LRCLIB as in [Normal]). */
        Refresh,

        /** "Search online": ask LRCLIB even if it said "not found" before. */
        SearchOnline,
    }

    private val mutex = Mutex()
    @Volatile private var lrclibBusyUntil = 0L

    /** The answer from the cache alone, or null when a source still has to be asked. */
    fun fromCache(track: LyricsTrack): LyricsAnswer? {
        val e = cache.read(track.songId) ?: return null
        val online = lrclib() != null
        if (needsServer(e, Mode.Normal) || needsLrclib(track, e, Mode.Normal, online)) return null
        return answerOf(e, online)
    }

    /** True when LRCLIB could still be asked for this track ("Search online" has a point). */
    fun canSearchOnline(track: LyricsTrack): Boolean = lrclib() != null && track.lookable

    suspend fun resolve(track: LyricsTrack, mode: Mode = Mode.Normal): LyricsAnswer = mutex.withLock {
        var e = cache.read(track.songId) ?: LyricsEntry()
        val before = e
        var failure: Throwable? = null
        var busy = false

        if (needsServer(e, mode)) {
            try {
                val chosen = ServerLyrics.choose(server(track.songId), deviceLanguages())
                e = e.copy(server = chosen, serverAt = cache.now())
            } catch (c: CancellationException) {
                throw c
            } catch (t: Throwable) {
                if (isNetworkError(t)) failure = t else e = e.copy(server = null, serverAt = cache.now())
            }
        }

        val client = lrclib()
        if (needsLrclib(track, e, mode, client != null) && client != null) {
            if (cache.now() < lrclibBusyUntil) {
                if (failure == null) failure = LrclibError.Busy(null)
                busy = true
            } else {
                try {
                    e = e.withLrclib(client.lookup(track), cache.now())
                } catch (c: CancellationException) {
                    throw c
                } catch (b: LrclibError.Busy) {
                    lrclibBusyUntil = cache.now() + (b.retryAfterSec ?: 60).coerceIn(1, 3600) * 1000
                    busy = true
                    if (failure == null) failure = b
                } catch (t: LrclibError) {
                    if (failure == null) failure = t
                }
            }
        }

        if (e != before) cache.write(track.songId, e)
        val answer = answerOf(e, client != null)
        // Something to show wins over an error; "nothing" is only trusted when every source answered.
        if (answer is LyricsAnswer.Found || failure == null) return@withLock answer
        throw LyricsLookupError(busy, failure)
    }

    /** With looking up online off, what LRCLIB said before is not shown either (as on the desktop). */
    private fun answerOf(e: LyricsEntry, online: Boolean): LyricsAnswer =
        SourceOrder.pick(e.server, if (online) e.lrclibAnswer else null)

    private fun needsServer(e: LyricsEntry, mode: Mode): Boolean = when (mode) {
        Mode.Refresh -> true
        Mode.SearchOnline -> e.serverAt <= 0
        // The server's "none" expires, unless LRCLIB found lyrics: found lyrics are kept (a pull asks again).
        Mode.Normal -> e.serverAt <= 0 || (e.server == null && e.lrclibKind != LyricsEntry.KIND_FOUND && cache.isStale(e.serverAt))
    }

    private fun needsLrclib(track: LyricsTrack, e: LyricsEntry, mode: Mode, online: Boolean): Boolean {
        if (!online || !track.lookable) return false
        if (e.server?.synced == true) return false // nothing LRCLIB has would be shown
        return when (mode) {
            Mode.SearchOnline -> true
            else -> e.lrclibKind == null || (e.lrclibKind == LyricsEntry.KIND_NOT_FOUND && cache.isStale(e.lrclibAt))
        }
    }
}
