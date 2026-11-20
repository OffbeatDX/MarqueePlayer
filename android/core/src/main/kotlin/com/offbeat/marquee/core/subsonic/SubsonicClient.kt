package com.offbeat.marquee.core.subsonic

import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.serialization.DeserializationStrategy
import kotlinx.serialization.SerializationException
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.intOrNull
import okhttp3.Call
import okhttp3.Callback
import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.Interceptor
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import java.io.IOException
import java.io.InterruptedIOException
import java.net.ProtocolException
import java.net.UnknownHostException
import java.util.concurrent.TimeUnit
import javax.net.ssl.SSLException
import kotlin.coroutines.resumeWithException

/**
 * A Subsonic / OpenSubsonic client with token auth.
 *
 * In [readOnly] mode only [ReadOnlyPolicy.ALLOWED] endpoints can be called: anything else throws
 * [SubsonicError.RefusedReadOnly] before a request exists, and the HTTP client carries a
 * [ReadOnlyInterceptor] as a second guard.
 *
 * URLs built here carry the token: never log them.
 */
class SubsonicClient(
    val credentials: SubsonicCredentials,
    val readOnly: Boolean,
    httpClient: OkHttpClient = defaultHttpClient(),
    private val clientName: String = CLIENT_NAME,
) {
    private val base: HttpUrl = normalizeUrl(credentials.url)
        ?: throw SubsonicError.InvalidUrl(hasCredentials = hasUrlCredentials(credentials.url))

    /**
     * The HTTP client every call goes through (API, streams, downloads, covers): automatic
     * redirects are off and [SameOriginRedirectInterceptor] allows only same-origin redirects
     * (same scheme, host and port, up to [SameOriginRedirectInterceptor.MAX_REDIRECTS] hops) so
     * the signed query never leaves the server. Read-only mode adds [ReadOnlyInterceptor] right
     * after it so a same-origin redirect cannot bypass the allowlist either.
     */
    val httpClient: OkHttpClient = httpClient.newBuilder().apply {
        followRedirects(false)
        followSslRedirects(false)
        val redirectIndex = interceptors().indexOfFirst { it is SameOriginRedirectInterceptor }
        val afterRedirect = if (redirectIndex >= 0) {
            redirectIndex + 1
        } else {
            interceptors().add(0, SameOriginRedirectInterceptor())
            1
        }
        if (readOnly && interceptors().none { it is ReadOnlyInterceptor }) {
            interceptors().add(afterRedirect, ReadOnlyInterceptor())
        }
    }.build()

    // ---- Calls --------------------------------------------------------------

    suspend fun ping(): ServerInfo = request("ping") { envelope -> json.decodeFromJsonElement(ServerInfo.serializer(), envelope) }

    suspend fun getOpenSubsonicExtensions(): List<OpenSubsonicExtension> =
        get("getOpenSubsonicExtensions", "openSubsonicExtensions", ListSerializer(OpenSubsonicExtension.serializer())) ?: emptyList()

    suspend fun getScanStatus(): ScanStatus = get("getScanStatus", "scanStatus", ScanStatus.serializer()) ?: ScanStatus()

    suspend fun getUser(username: String = credentials.user): User =
        get("getUser", "user", User.serializer(), "username" to username) ?: throw SubsonicError.NotSubsonic("no user in response")

    suspend fun getAlbumList2(
        type: AlbumListType,
        size: Int = 10,
        offset: Int = 0,
        fromYear: Int? = null,
        toYear: Int? = null,
        genre: String? = null,
        musicFolderId: String? = null,
    ): List<Album> = get(
        "getAlbumList2", "albumList2", AlbumListWrapper.serializer(),
        "type" to type.wire, "size" to size.toString(), "offset" to offset.toString(),
        "fromYear" to fromYear?.toString(), "toYear" to toYear?.toString(),
        "genre" to genre, "musicFolderId" to musicFolderId,
    )?.album.orEmpty()

    suspend fun getAlbum(id: String): Album =
        get("getAlbum", "album", Album.serializer(), "id" to id) ?: throw SubsonicError.NotSubsonic("no album in response")

    suspend fun getArtists(musicFolderId: String? = null): ArtistsIndex =
        get("getArtists", "artists", ArtistsIndex.serializer(), "musicFolderId" to musicFolderId) ?: ArtistsIndex()

    suspend fun getArtist(id: String): Artist =
        get("getArtist", "artist", Artist.serializer(), "id" to id) ?: throw SubsonicError.NotSubsonic("no artist in response")

    /** Artist details: biography and image URLs (Navidrome's are its own `/share/img/...` links). */
    suspend fun getArtistInfo2(id: String, count: Int = 0): ArtistInfo =
        get("getArtistInfo2", "artistInfo2", ArtistInfo.serializer(), "id" to id, "count" to count.toString()) ?: ArtistInfo()

    /** The artist's most popular songs, by name (Navidrome asks Last.fm; empty without an agent). */
    suspend fun getTopSongs(artist: String, count: Int = 50): List<Song> =
        get("getTopSongs", "topSongs", SongsWrapper.serializer(), "artist" to artist, "count" to count.toString())?.song.orEmpty()

    suspend fun search3(
        query: String,
        artistCount: Int = 20,
        albumCount: Int = 20,
        songCount: Int = 20,
        artistOffset: Int = 0,
        albumOffset: Int = 0,
        songOffset: Int = 0,
    ): SearchResult = get(
        "search3", "searchResult3", SearchResult.serializer(),
        "query" to query,
        "artistCount" to artistCount.toString(), "albumCount" to albumCount.toString(), "songCount" to songCount.toString(),
        "artistOffset" to artistOffset.toString(), "albumOffset" to albumOffset.toString(), "songOffset" to songOffset.toString(),
    ) ?: SearchResult()

    suspend fun getPlaylists(): List<Playlist> = get("getPlaylists", "playlists", PlaylistsWrapper.serializer())?.playlist.orEmpty()

    suspend fun getPlaylist(id: String): Playlist =
        get("getPlaylist", "playlist", Playlist.serializer(), "id" to id) ?: throw SubsonicError.NotSubsonic("no playlist in response")

    /** Genres with their album and song counts. */
    suspend fun getGenres(): List<Genre> = get("getGenres", "genres", GenresWrapper.serializer())?.genre.orEmpty()

    /** Songs of a genre (Navidrome caps [count] at 500). */
    suspend fun getSongsByGenre(genre: String, count: Int = 100, offset: Int = 0): List<Song> = get(
        "getSongsByGenre", "songsByGenre", SongsWrapper.serializer(),
        "genre" to genre, "count" to count.toString(), "offset" to offset.toString(),
    )?.song.orEmpty()

    /**
     * OpenSubsonic `getLyricsBySongId` (the `songLyrics` extension): every lyrics entry the server
     * has for the song (synced and plain, maybe several languages); empty when it has none.
     */
    suspend fun getLyricsBySongId(id: String): List<com.offbeat.marquee.core.lyrics.StructuredLyrics> =
        get("getLyricsBySongId", "lyricsList", LyricsListWrapper.serializer(), "id" to id)?.structuredLyrics.orEmpty()

    /** The user's favourites: artists, albums and songs. */
    suspend fun getStarred2(): Starred = get("getStarred2", "starred2", Starred.serializer()) ?: Starred()

    // Writes. Refused in read-only mode; listed here so the refusal is tested on real call sites.

    /** Rates a song, album or artist 1-5; 0 removes the rating. */
    suspend fun setRating(id: String, rating: Int) {
        require(rating in 0..5)
        request("setRating", "id" to id, "rating" to rating.toString()) { }
    }

    /** Adds songs to the end of a playlist the user owns. */
    suspend fun updatePlaylist(playlistId: String, songIdsToAdd: List<String>) {
        val params = buildList {
            add("playlistId" to playlistId)
            songIdsToAdd.forEach { add("songIdToAdd" to it) }
        }
        request("updatePlaylist", *params.toTypedArray<Pair<String, String?>>()) { }
    }

    suspend fun star(id: String? = null, albumId: String? = null, artistId: String? = null) {
        request("star", "id" to id, "albumId" to albumId, "artistId" to artistId) { }
    }

    suspend fun unstar(id: String? = null, albumId: String? = null, artistId: String? = null) {
        request("unstar", "id" to id, "albumId" to albumId, "artistId" to artistId) { }
    }

    suspend fun scrobble(id: String, timeMillis: Long? = null, submission: Boolean = true) {
        request("scrobble", "id" to id, "time" to timeMillis?.toString(), "submission" to submission.toString()) { }
    }

    /** Several submissions in one call: `id` and `time` repeated, in the same order (Navidrome takes this). */
    suspend fun scrobble(ids: List<String>, timesMillis: List<Long>) {
        require(ids.isNotEmpty() && ids.size == timesMillis.size)
        val params = buildList {
            ids.forEach { add("id" to it) }
            timesMillis.forEach { add("time" to it.toString()) }
            add("submission" to "true")
        }
        request("scrobble", *params.toTypedArray<Pair<String, String?>>()) { }
    }

    // ---- URL builders (for images and the player; they carry auth) ------------

    /** A cover at [size] pixels on its long side; pass the size it is drawn at. */
    fun coverArtUrl(id: String, size: Int? = null): String = url("getCoverArt", "id" to id, "size" to size?.toString())

    /**
     * A stream URL. [format] "raw" asks for the original; "opus", "mp3" etc. transcode at up to
     * [maxBitRate] kbps (0 or null = no limit).
     */
    fun streamUrl(id: String, format: String? = null, maxBitRate: Int? = null, timeOffsetSeconds: Int? = null): String =
        url("stream", "id" to id, "format" to format, "maxBitRate" to maxBitRate?.toString(), "timeOffset" to timeOffsetSeconds?.toString())

    // ---- Plumbing -----------------------------------------------------------

    private fun url(endpoint: String, vararg params: Pair<String, String?>): String = buildUrl(endpoint, params).toString()

    private fun buildUrl(endpoint: String, params: Array<out Pair<String, String?>>): HttpUrl {
        if (readOnly) ReadOnlyPolicy.check(endpoint)
        val b = base.newBuilder()
            .addPathSegment("rest")
            .addPathSegment("$endpoint.view")
            .addQueryParameter("u", credentials.user)
            .addQueryParameter("t", credentials.token)
            .addQueryParameter("s", credentials.salt)
            .addQueryParameter("v", API_VERSION)
            .addQueryParameter("c", clientName)
            .addQueryParameter("f", "json")
        for ((k, v) in params) if (v != null) b.addQueryParameter(k, v)
        return b.build()
    }

    private suspend fun <T> get(
        endpoint: String,
        key: String,
        serializer: DeserializationStrategy<T>,
        vararg params: Pair<String, String?>,
    ): T? = request(endpoint, *params) { envelope ->
        envelope[key]?.let { json.decodeFromJsonElement(serializer, it) }
    }

    private suspend fun <T> request(
        endpoint: String,
        vararg params: Pair<String, String?>,
        parse: (JsonObject) -> T,
    ): T {
        val url = buildUrl(endpoint, params) // throws RefusedReadOnly before anything is sent
        val response = try {
            httpClient.newCall(Request.Builder().url(url).get().build()).await()
        } catch (e: ReadOnlyRefusal) {
            throw SubsonicError.RefusedReadOnly(e.endpoint)
        } catch (e: IOException) {
            throw networkError(e)
        }
        val body = response.use { r ->
            val text = try {
                r.body?.string().orEmpty()
            } catch (e: IOException) {
                throw networkError(e)
            }
            Triple(r.code, r.isSuccessful, text)
        }
        val (status, ok, text) = body
        val envelope = parseEnvelope(text)
        if (envelope == null) {
            if (status in 500..599) throw SubsonicError.Server(null, status, null)
            throw SubsonicError.NotSubsonic(if (ok) "unexpected response" else "HTTP $status")
        }
        if ((envelope["status"] as? JsonPrimitive)?.content != "ok") throw subsonicError(envelope, status)
        return try {
            parse(envelope)
        } catch (e: SerializationException) {
            throw SubsonicError.NotSubsonic("malformed $endpoint response")
        } catch (e: IllegalArgumentException) {
            throw SubsonicError.NotSubsonic("malformed $endpoint response")
        }
    }

    private fun parseEnvelope(text: String): JsonObject? = try {
        (json.parseToJsonElement(text) as? JsonObject)?.get("subsonic-response") as? JsonObject
    } catch (e: SerializationException) {
        null
    }

    private fun subsonicError(envelope: JsonObject, httpStatus: Int): SubsonicError {
        val error = envelope["error"] as? JsonObject
        val code = (error?.get("code") as? JsonPrimitive)?.intOrNull
        val message = (error?.get("message") as? JsonPrimitive)?.content
        return if (code in AUTH_CODES) SubsonicError.AuthFailed(code!!, message)
        else SubsonicError.Server(code, httpStatus.takeIf { it !in 200..299 }, message)
    }

    private fun networkError(e: IOException): SubsonicError.Network = SubsonicError.Network(
        when (e) {
            is UnknownHostException -> SubsonicError.Network.Kind.UnknownHost
            is InterruptedIOException -> SubsonicError.Network.Kind.Timeout
            is SSLException -> SubsonicError.Network.Kind.Tls
            else -> SubsonicError.Network.Kind.Other
        },
        e,
    )

    @kotlinx.serialization.Serializable
    private data class AlbumListWrapper(val album: List<Album> = emptyList())

    @kotlinx.serialization.Serializable
    private data class SongsWrapper(val song: List<Song> = emptyList())

    @kotlinx.serialization.Serializable
    private data class GenresWrapper(val genre: List<Genre> = emptyList())

    @kotlinx.serialization.Serializable
    private data class LyricsListWrapper(val structuredLyrics: List<com.offbeat.marquee.core.lyrics.StructuredLyrics> = emptyList())

    @kotlinx.serialization.Serializable
    private data class PlaylistsWrapper(val playlist: List<Playlist> = emptyList())

    companion object {
        const val API_VERSION = "1.16.1"
        const val CLIENT_NAME = "MarqueePlayer"

        /** 40 wrong credentials, 41 token auth unsupported, 42-44 other auth mechanism problems. */
        private val AUTH_CODES = setOf(40, 41, 42, 43, 44)

        internal val json = Json {
            ignoreUnknownKeys = true
            coerceInputValues = true
            isLenient = true
        }

        fun defaultHttpClient(): OkHttpClient = OkHttpClient.Builder()
            .connectTimeout(15, TimeUnit.SECONDS)
            .readTimeout(30, TimeUnit.SECONDS)
            .followRedirects(false)
            .followSslRedirects(false)
            .addInterceptor(SameOriginRedirectInterceptor())
            .build()

        private fun parseUrl(input: String): HttpUrl? {
            var s = input.trim()
            if (s.isEmpty()) return null
            if (!s.contains("://")) s = "https://$s"
            s = s.trimEnd('/')
            if (s.endsWith("/rest")) s = s.removeSuffix("/rest")
            val url = s.toHttpUrlOrNull() ?: return null
            if (url.host.isEmpty()) return null
            return url
        }

        /** True when [input] puts a username or password in the address (e.g. `https://me:secret@host`). */
        fun hasUrlCredentials(input: String): Boolean {
            val url = parseUrl(input) ?: return false
            return url.username.isNotEmpty() || url.password.isNotEmpty()
        }

        /**
         * The base URL from what the user typed: https:// added when there is no scheme, a
         * trailing slash or `/rest` removed. Null when it cannot be an HTTP URL or carries a
         * username or password.
         */
        fun normalizeUrl(input: String): HttpUrl? {
            val url = parseUrl(input) ?: return null
            if (url.username.isNotEmpty() || url.password.isNotEmpty()) return null
            return url.newBuilder().query(null).fragment(null).build()
        }
    }
}

/**
 * Follows HTTP redirects only within the request's own origin (same scheme, host and port), up to
 * [maxRedirects] hops. Any redirect to another host or port, a different scheme (such as https to
 * http), a URL carrying credentials, or past the hop limit is refused so a signed query (`u`, `t`,
 * `s`) never reaches another server or goes out over plain http.
 *
 * Used with `followRedirects(false)` and `followSslRedirects(false)` on the OkHttp client.
 */
class SameOriginRedirectInterceptor(private val maxRedirects: Int = MAX_REDIRECTS) : Interceptor {
    override fun intercept(chain: Interceptor.Chain): Response {
        var current = chain.request()
        val origin = current.url
        var response = chain.proceed(current)
        var hops = 0
        while (response.code in REDIRECT_CODES) {
            val location = response.header("Location") ?: return response
            val target = current.url.resolve(location)
            if (target == null || !sameOrigin(origin, target) || target.username.isNotEmpty() || target.password.isNotEmpty()) {
                response.close()
                throw ProtocolException("Refused redirect to another server or scheme")
            }
            if (hops >= maxRedirects) {
                response.close()
                throw ProtocolException("Too many redirects")
            }
            val code = response.code
            response.close()
            hops++
            current = followUp(current, code, target)
            response = chain.proceed(current)
        }
        return response
    }

    private fun followUp(request: Request, statusCode: Int, target: HttpUrl): Request {
        val builder = request.newBuilder().url(target)
        val method = request.method
        if (method != "GET" && method != "HEAD" && (statusCode == 303 || ((statusCode == 301 || statusCode == 302) && method == "POST"))) {
            builder.method("GET", null)
            builder.removeHeader("Transfer-Encoding")
            builder.removeHeader("Content-Length")
            builder.removeHeader("Content-Type")
        }
        return builder.build()
    }

    companion object {
        const val MAX_REDIRECTS = 5
        private val REDIRECT_CODES = setOf(300, 301, 302, 303, 307, 308)

        internal fun sameOrigin(a: HttpUrl, b: HttpUrl): Boolean =
            a.scheme == b.scheme && a.host == b.host && a.port == b.port
    }
}

/** The call, sent asynchronously and cancelled with the coroutine. */
internal suspend fun Call.await(): Response = suspendCancellableCoroutine { cont ->
    enqueue(object : Callback {
        override fun onResponse(call: Call, response: Response) {
            cont.resume(response) { response.close() }
        }
        override fun onFailure(call: Call, e: IOException) {
            if (!cont.isCancelled) cont.resumeWithException(e)
        }
    })
    cont.invokeOnCancellation { runCatching { cancel() } }
}
