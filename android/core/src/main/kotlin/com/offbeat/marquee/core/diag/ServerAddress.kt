package com.offbeat.marquee.core.diag

import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import java.net.IDN

/**
 * The server as the release build's Settings names it: the host, the port when it isn't the
 * scheme's default, and the path when Navidrome isn't at the root, e.g. `music.example.com`,
 * `music.example.com:4533`, `example.com/navidrome`. Never the scheme, a user name or password in
 * the URL, a query (a token could sit there) or a fragment.
 */
object ServerAddress {
    fun display(url: String): String? {
        val s = url.trim()
        if (s.isEmpty()) return null
        val u = s.toHttpUrlOrNull() ?: (if ("://" !in s) "https://$s".toHttpUrlOrNull() else null) ?: return null
        if (u.host.isEmpty()) return null
        return buildString {
            append(displayHost(u.host))
            if (u.port != HttpUrl.defaultPort(u.scheme)) append(':').append(u.port)
            val segments = u.pathSegments.filter { it.isNotEmpty() }
            // The API itself (/rest, /rest/ping…) is under the server's base: it's left out.
            val path = segments.takeWhile { it != "rest" }
            if (path.isNotEmpty()) append('/').append(path.joinToString("/"))
        }
    }

    /** The host as people write it: an internationalised name in its own letters, IPv6 in brackets. */
    fun displayHost(host: String): String = when {
        ':' in host -> "[$host]"
        else -> runCatching { IDN.toUnicode(host) }.getOrDefault(host)
    }

    /** Just the host (no port, no path): what the scrubber hides. Null when [url] isn't an HTTP URL. */
    fun host(url: String): String? {
        val s = url.trim()
        val u = s.toHttpUrlOrNull() ?: (if ("://" !in s) "https://$s".toHttpUrlOrNull() else null)
        return u?.host?.takeIf { it.isNotEmpty() }
    }
}

/** The release build's account line under "Signed in as …": `music.example.com · Navidrome 0.64.2`. */
fun accountSummary(url: String, server: String?): String =
    listOfNotNull(ServerAddress.display(url), server?.takeIf { it.isNotBlank() }).joinToString(" · ").ifEmpty { "Signed in" }
