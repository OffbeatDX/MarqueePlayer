package com.offbeat.marquee.core.subsonic

import okhttp3.Interceptor
import okhttp3.Response
import java.io.IOException

/**
 * Read-only mode: the only endpoints a test run may call (android/docs/DESIGN.md, "Testing rules").
 * Anything else is refused before a request is built, and again by [ReadOnlyInterceptor] in case
 * a URL is built some other way.
 */
object ReadOnlyPolicy {
    val ALLOWED: Set<String> = setOf(
        "ping", "getOpenSubsonicExtensions", "getMusicFolders", "getArtists", "getArtist",
        "getAlbum", "getSong", "getArtistInfo2", "getAlbumInfo2", "getTopSongs",
        "getSimilarSongs2", "getAlbumList2", "getRandomSongs", "getSongsByGenre", "getStarred2",
        "getGenres", "search3", "getPlaylists", "getPlaylist", "getPlayQueue", "getLyricsBySongId",
        "getCoverArt", "stream", "download", "getScanStatus", "getUser",
    )

    fun isAllowed(endpoint: String): Boolean = endpoint.removeSuffix(".view") in ALLOWED

    fun check(endpoint: String) {
        if (!isAllowed(endpoint)) throw SubsonicError.RefusedReadOnly(endpoint.removeSuffix(".view"))
    }
}

/**
 * Refuses any request to a `rest/<endpoint>` outside the allowlist, so it never leaves the device.
 * It throws an [IOException] ([ReadOnlyRefusal]) because that is what OkHttp callers (and later
 * the player's data source) expect from a failed request; [SubsonicClient] turns it back into
 * [SubsonicError.RefusedReadOnly].
 */
class ReadOnlyInterceptor : Interceptor {
    override fun intercept(chain: Interceptor.Chain): Response {
        val segments = chain.request().url.pathSegments
        val restIndex = segments.lastIndexOf("rest")
        val endpoint = if (restIndex >= 0 && restIndex < segments.size - 1) segments[restIndex + 1] else ""
        if (!ReadOnlyPolicy.isAllowed(endpoint)) throw ReadOnlyRefusal(endpoint.removeSuffix(".view"))
        return chain.proceed(chain.request())
    }
}

class ReadOnlyRefusal(val endpoint: String) : IOException("Refused by read-only mode: $endpoint")
