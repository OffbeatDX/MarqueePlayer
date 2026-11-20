package com.offbeat.marquee.core.lyrics

import com.offbeat.marquee.core.subsonic.SameOriginRedirectInterceptor
import com.offbeat.marquee.core.subsonic.await
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.SerializationException
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.json.Json
import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.Interceptor
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import java.io.IOException
import kotlin.math.abs
import kotlin.math.roundToLong

/** What is looked up on LRCLIB: the song's own tags. [durationSec] in seconds. */
data class LyricsTrack(
    val songId: String,
    val artist: String,
    val title: String,
    val album: String,
    val durationSec: Double,
) {
    /**
     * Worth asking LRCLIB about: an artist, a title and a length it accepts (1 s to an hour; it
     * matches on the length, and without one a wrong edit could come back).
     */
    val lookable: Boolean get() = artist.isNotBlank() && title.isNotBlank() && durationSec in 1.0..3600.0
}

/** A lyrics record, as both LRCLIB calls return it. */
@Serializable
data class LrclibRecord(
    val id: Long = 0,
    @SerialName("trackName") val trackName: String = "",
    @SerialName("artistName") val artistName: String = "",
    @SerialName("albumName") val albumName: String? = null,
    val duration: Double = 0.0,
    val instrumental: Boolean = false,
    @SerialName("plainLyrics") val plainLyrics: String? = null,
    @SerialName("syncedLyrics") val syncedLyrics: String? = null,
) {
    internal val hasWords: Boolean get() = !syncedLyrics.isNullOrBlank() || !plainLyrics.isNullOrBlank()
    internal val hasSynced: Boolean get() = !syncedLyrics.isNullOrBlank()

    /** Synced lyrics when it has them and they parse with stamps, else the plain ones. */
    fun answer(): LyricsAnswer {
        if (instrumental) return LyricsAnswer.Instrumental
        syncedLyrics?.let { s ->
            val l = Lrc.parse(s, LyricsSource.Lrclib)
            if (l.synced && !l.isEmpty) return LyricsAnswer.Found(l)
        }
        plainLyrics?.let { p ->
            val l = Lrc.parse(p, LyricsSource.Lrclib)
            if (!l.isEmpty) {
                // Plain text with something that looks like a stamp is still shown plain.
                return LyricsAnswer.Found(
                    if (l.synced) l.copy(synced = false, offsetMs = 0, lines = l.lines.map { it.copy(startMs = null) }) else l,
                )
            }
        }
        return LyricsAnswer.NotFound
    }
}

sealed class LrclibError(message: String, cause: Throwable? = null) : Exception(message, cause) {
    /** No connection, DNS, timeout: offline for now. */
    class Network(cause: IOException) : LrclibError("network: ${cause.javaClass.simpleName}", cause)

    /** 429 or 503: wait [retryAfterSec] (when the service said) before asking again. */
    class Busy(val retryAfterSec: Long?) : LrclibError("busy")

    /** Another status, or a body that is not what the API returns. */
    class Server(message: String) : LrclibError(message)
}

/** The rule for taking a search result, ported from the desktop's `crates/lrclib/src/client.rs`. */
object LrclibMatch {
    /** How far a search result's length may be from the track's, in seconds. */
    const val DURATION_SLACK = 2.0

    /** Same artist and title after folding, length within the slack. */
    fun matches(t: LyricsTrack, r: LrclibRecord): Boolean =
        Fold.fold(t.artist) == Fold.fold(r.artistName) &&
            Fold.fold(t.title) == Fold.fold(r.trackName) &&
            abs(t.durationSec - r.duration) <= DURATION_SLACK

    /** A search result that may be shown for [t]: it matches, and has words (or is flagged instrumental). */
    fun acceptable(t: LyricsTrack, r: LrclibRecord): Boolean = matches(t, r) && (r.instrumental || r.hasWords)

    /** Of the acceptable results: the same album first, then synced, then the closest length. */
    fun best(t: LyricsTrack, results: List<LrclibRecord>): LrclibRecord? {
        val album = Fold.fold(t.album)
        return results.filter { acceptable(t, it) }.minWithOrNull(
            compareBy<LrclibRecord>(
                { if (it.albumName != null && Fold.fold(it.albumName) == album) 0 else 1 },
                { if (it.hasSynced) 0 else 1 },
                { abs(it.duration - t.durationSec) },
            ),
        )
    }
}

/**
 * The two LRCLIB calls (https://lrclib.net/docs): the exact lookup (`/api/get`: artist, title,
 * album, duration), then, when it finds nothing, the search (`/api/search` by title and artist),
 * whose results are taken only by [LrclibMatch]. A wrong lyric is worse than none.
 *
 * [baseUrl] is `https://lrclib.net` in the release build and a local mock in tests; it is its own
 * client because the Subsonic read-only guard refuses anything that is not a `rest/` endpoint.
 * [userAgent] is generic (`MarqueePlayer/<version> (Android)`), never anything personal.
 */
class LrclibClient(
    private val httpClient: OkHttpClient,
    baseUrl: String,
    private val userAgent: String,
) {
    private val base: HttpUrl = baseUrl.trimEnd('/').toHttpUrlOrNull() ?: throw IllegalArgumentException("bad LRCLIB URL")

    /** The host it talks to (for the log: never the track). */
    val host: String get() = base.host

    /** `GET /api/get`: the record for exactly this track, or null (404). */
    suspend fun exact(t: LyricsTrack): LrclibRecord? {
        val url = base.newBuilder().addPathSegments("api/get")
            .addQueryParameter("artist_name", t.artist)
            .addQueryParameter("track_name", t.title)
            .apply { if (t.album.isNotBlank()) addQueryParameter("album_name", t.album) }
            .addQueryParameter("duration", t.durationSec.roundToLong().toString())
            .build()
        val (code, body) = get(url)
        return when (code) {
            200 -> decode(body) { json.decodeFromString(LrclibRecord.serializer(), it) }
            404 -> null
            else -> throw LrclibError.Server("HTTP $code")
        }
    }

    /** `GET /api/search` by title and artist (structured, not free text): up to 20 records. */
    suspend fun search(t: LyricsTrack): List<LrclibRecord> {
        val url = base.newBuilder().addPathSegments("api/search")
            .addQueryParameter("track_name", t.title)
            .addQueryParameter("artist_name", t.artist)
            .build()
        val (code, body) = get(url)
        return when (code) {
            200 -> decode(body) { json.decodeFromString(ListSerializer(LrclibRecord.serializer()), it) }
            404 -> emptyList()
            else -> throw LrclibError.Server("HTTP $code")
        }
    }

    /** The exact lookup, then the search. */
    suspend fun lookup(t: LyricsTrack): LyricsAnswer {
        exact(t)?.let { r ->
            val a = r.answer()
            if (a != LyricsAnswer.NotFound) return a
        }
        return LrclibMatch.best(t, search(t))?.answer() ?: LyricsAnswer.NotFound
    }

    private inline fun <T> decode(body: String, f: (String) -> T): T = try {
        f(body)
    } catch (e: SerializationException) {
        throw LrclibError.Server("unexpected answer")
    } catch (e: IllegalArgumentException) {
        throw LrclibError.Server("unexpected answer")
    }

    private suspend fun get(url: HttpUrl): Pair<Int, String> {
        val request = Request.Builder().url(url).header("User-Agent", userAgent).header("Accept", "application/json").get().build()
        val response = try {
            httpClient.newCall(request).await()
        } catch (e: IOException) {
            throw LrclibError.Network(e)
        }
        return response.use { r ->
            if (r.code == 429 || r.code == 503) throw LrclibError.Busy(r.header("Retry-After")?.trim()?.toLongOrNull())
            val text = try {
                r.body?.string().orEmpty()
            } catch (e: IOException) {
                throw LrclibError.Network(e)
            }
            r.code to text
        }
    }

    companion object {
        const val PUBLIC_URL = "https://lrclib.net"

        private val json = Json {
            ignoreUnknownKeys = true
            coerceInputValues = true
            isLenient = true
        }

        fun userAgent(version: String): String = "MarqueePlayer/$version (Android)"
    }
}

/**
 * Read-only (test) builds: LRCLIB requests may only go to this device's loopback (a local stand-in
 * behind `adb reverse`), never to lrclib.net or anywhere else. Refused before anything is sent.
 */
class LoopbackOnly(private val onRefused: () -> Unit = {}) : Interceptor {
    override fun intercept(chain: Interceptor.Chain): Response {
        val host = chain.request().url.host
        if (host != "127.0.0.1" && host != "localhost") {
            onRefused()
            throw IOException("Refused by read-only mode: LRCLIB is local only in test builds")
        }
        return chain.proceed(chain.request())
    }
}

/**
 * Sets up a read-only (test) build's LRCLIB client so every request, redirects included, stays on
 * the loopback: [LoopbackOnly] checks the first request, and OkHttp's own redirect following (which
 * would run after it, unchecked) is replaced by [SameOriginRedirectInterceptor], which follows a
 * redirect only to the same scheme, host and port and refuses any other before a DNS lookup or a
 * connection. The stand-in never needs a redirect to another host.
 */
fun OkHttpClient.Builder.lrclibLoopbackOnly(onRefused: () -> Unit = {}): OkHttpClient.Builder =
    followRedirects(false)
        .followSslRedirects(false)
        .addInterceptor(LoopbackOnly(onRefused))
        .addInterceptor(SameOriginRedirectInterceptor())
