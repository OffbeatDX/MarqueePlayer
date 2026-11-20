package com.offbeat.marquee.core.art

import java.net.URLDecoder
import java.net.URLEncoder

/**
 * Cover art as other apps load it: `content://<authority>/cover/<coverId>/<size>`, served by the
 * app's cover provider. Android Auto, the system's media controls and the like load an item's
 * `artworkUri` themselves, so it must never be a `getCoverArt` URL (that carries the token). This
 * one names only the cover and a size bucket ([CoverSize]); nothing about the server or the account.
 */
object CoverUris {
    const val SEGMENT = "cover"

    private val SIZES = setOf(CoverSize.THUMB, CoverSize.GRID, CoverSize.HEADER, CoverSize.FULL)

    data class Request(val coverId: String, val size: Int)

    /** The URI as a string (percent-encoded id); `Uri.parse` takes it as is. */
    fun uri(authority: String, coverId: String, size: Int): String {
        require(size in SIZES) { "not a cover size bucket: $size" }
        return "content://$authority/$SEGMENT/${enc(coverId)}/$size"
    }

    /**
     * The request back from a URI's decoded path segments (`Uri.pathSegments`). Anything else (another
     * path, a size that isn't a bucket, an empty id) is refused, so the provider can't be made to fetch
     * arbitrary sizes or paths.
     */
    fun parse(segments: List<String>): Request? {
        if (segments.size != 3 || segments[0] != SEGMENT) return null
        val id = segments[1]
        val size = segments[2].toIntOrNull() ?: return null
        if (id.isBlank() || size !in SIZES) return null
        return Request(id, size)
    }

    /** The cover id of one of these URIs given as a string, or null. */
    fun coverIdOf(uri: String?, authority: String): String? {
        val prefix = "content://$authority/"
        if (uri == null || !uri.startsWith(prefix)) return null
        val segments = uri.removePrefix(prefix).substringBefore('?').split('/').map { runCatching { dec(it) }.getOrDefault(it) }
        return parse(segments)?.coverId
    }

    // The String charset overloads: the Charset ones are Java 10, on Android only from API 33
    // (minSdk is 31), and crashed Android 12 with NoSuchMethodError.
    private fun enc(s: String) = URLEncoder.encode(s, "UTF-8").replace("+", "%20")
    private fun dec(s: String) = URLDecoder.decode(s, "UTF-8")
}
