package com.offbeat.marquee.player.data

import android.content.Context
import android.os.LocaleList
import com.offbeat.marquee.player.diag.AppLog
import com.offbeat.marquee.core.lyrics.lrclibLoopbackOnly
import com.offbeat.marquee.core.lyrics.LrclibClient
import com.offbeat.marquee.core.lyrics.LyricsAnswer
import com.offbeat.marquee.core.lyrics.LyricsCache
import com.offbeat.marquee.core.lyrics.LyricsResolver
import com.offbeat.marquee.core.lyrics.LyricsTrack
import com.offbeat.marquee.core.subsonic.Song
import com.offbeat.marquee.core.subsonic.SubsonicError
import com.offbeat.marquee.player.BuildConfig
import com.offbeat.marquee.player.session.AppSession
import com.offbeat.marquee.player.session.LrclibTestEndpoint
import com.offbeat.marquee.player.settings.UiSettings
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import okhttp3.OkHttpClient
import java.io.File
import java.util.concurrent.TimeUnit

/** Why "Search online" can or cannot be used. */
enum class OnlineLyrics {
    Available,

    /** Off in Settings. */
    OffInSettings,

    /** A test build: LRCLIB is never contacted (unless a local stand-in was pushed). */
    OffInThisBuild,
}

/**
 * Lyrics for the app: the server's (`getLyricsBySongId`, through the session's client and its
 * read-only guard), then LRCLIB's, cached on disk by song id ([LyricsCache], a few MB, least
 * recently used out). Asked only by the lyrics view while it is open (see `LyricsViewModel`).
 *
 * LRCLIB has its own OkHttp client: the Subsonic read-only guard refuses anything that is not a
 * `rest/` endpoint. It shares the session client's connection pool and dispatcher. In read-only
 * builds it also carries [lrclibLoopbackOnly] (redirects included), so nothing but a local stand-in can ever be reached.
 */
object LyricsRepo {
    private const val TAG = "MarqueeLyrics"
    private lateinit var app: Context
    private val cache by lazy { LyricsCache(File(app.cacheDir, "lyrics")) }
    private val testEndpoint by lazy { LrclibTestEndpoint.load(app) }

    @Volatile private var lrclibFor: Pair<OkHttpClient, LrclibClient>? = null

    fun init(context: Context) {
        app = context.applicationContext
    }

    /** Where LRCLIB is in this build: the public service in release, a pushed loopback stand-in or nothing in test builds. */
    private fun endpoint(): String? = BuildConfig.LRCLIB_URL.ifEmpty { null } ?: testEndpoint

    fun online(): OnlineLyrics = when {
        endpoint() == null -> OnlineLyrics.OffInThisBuild
        !UiSettings.lyricsOnline.value -> OnlineLyrics.OffInSettings
        else -> OnlineLyrics.Available
    }

    /** True in a test build that has the local stand-in (the Settings note says so). */
    val usesTestStandIn: Boolean get() = BuildConfig.LRCLIB_URL.isEmpty() && testEndpoint != null

    private fun lrclib(): LrclibClient? {
        if (online() != OnlineLyrics.Available) return null
        val session = AppSession.current?.httpClient ?: return null
        lrclibFor?.let { (owner, client) -> if (owner === session) return client }
        val http = OkHttpClient.Builder()
            .connectionPool(session.connectionPool)
            .dispatcher(session.dispatcher)
            .connectTimeout(10, TimeUnit.SECONDS)
            .readTimeout(10, TimeUnit.SECONDS)
            .apply { if (BuildConfig.READ_ONLY) lrclibLoopbackOnly { AppLog.w(TAG, "Refused by read-only mode: a lyrics request to a non-loopback host") } }
            .addInterceptor { chain ->
                // The host and path only: never the track or the query.
                AppLog.i(TAG, "lrclib ${chain.request().url.host}${chain.request().url.encodedPath}")
                chain.proceed(chain.request())
            }
            .build()
        val client = LrclibClient(http, endpoint()!!, LrclibClient.userAgent(BuildConfig.VERSION_NAME.removeSuffix("-debug").removeSuffix("-bench")))
        lrclibFor = session to client
        return client
    }

    private val resolver by lazy {
        LyricsResolver(
            cache = cache,
            server = { id ->
                val c = AppSession.current ?: throw SubsonicError.Network(SubsonicError.Network.Kind.Other, null)
                c.getLyricsBySongId(id).also { AppLog.i(TAG, "server lyrics for $id: ${it.size} entries") }
            },
            lrclib = ::lrclib,
            deviceLanguages = ::deviceLanguages,
            isNetworkError = { it is SubsonicError.Network || it is SubsonicError.RefusedReadOnly },
        )
    }

    private fun deviceLanguages(): List<String> {
        val locales = LocaleList.getDefault()
        return buildList {
            for (i in 0 until locales.size()) {
                val l = locales[i]
                add(l.language)
                runCatching { l.isO3Language }.getOrNull()?.let(::add)
            }
        }.distinct()
    }

    suspend fun fromCache(track: LyricsTrack): LyricsAnswer? = withContext(Dispatchers.IO) { resolver.fromCache(track) }

    suspend fun resolve(track: LyricsTrack, mode: LyricsResolver.Mode = LyricsResolver.Mode.Normal): LyricsAnswer =
        withContext(Dispatchers.IO) { resolver.resolve(track, mode) }

    fun canSearchOnline(track: LyricsTrack): Boolean = resolver.canSearchOnline(track)

    /** Sign out: what was kept from this server goes with it. */
    fun clear() {
        if (::app.isInitialized) cache.clear()
    }

    fun trackOf(song: Song, fallbackDurationMs: Long = 0): LyricsTrack = LyricsTrack(
        songId = song.id,
        artist = song.artist.orEmpty(),
        title = song.title,
        album = song.album.orEmpty(),
        durationSec = song.duration?.toDouble()?.takeIf { it > 0 } ?: (fallbackDurationMs / 1000.0),
    )
}
