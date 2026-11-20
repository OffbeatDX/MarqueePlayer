package com.offbeat.marquee.core.diag

import java.net.IDN
import java.net.URLEncoder
import java.util.regex.Matcher
import java.util.regex.Pattern

/**
 * Takes what identifies the account out of a log line or a diagnostics report, so a friend can
 * share it without handing over their sign-in:
 *
 * 1. whole URLs (`https://…`, also percent-encoded `https%3A%2F%2F…`) become `<url>`: a stream or
 *    cover URL carries the user, salt and token in its query;
 * 2. the Subsonic auth parameters left in any other text (`u=`, `t=`, `s=`, `p=`, after `?`, `&`
 *    or their encoded forms) lose their values;
 * 3. the token, the salt, the user name and the server's host (and its domain) are replaced
 *    wherever they appear, ignoring case, also percent-encoded and, for the host, in its
 *    punycode or Unicode form, and so is the address a socket error gives after the host
 *    (`host/203.0.113.5:443`).
 *
 * A user name shorter than three characters is only replaced as a whole word, so a user called
 * "al" doesn't take every "al" out of the log. Immutable; [plus] adds another account's values.
 */
class Scrubber private constructor(private val secrets: List<Secret>) {

    private data class Secret(val value: String, val label: String, val wholeWord: Boolean)

    private val secretPattern: Pattern? = secrets
        .flatMap { s -> variants(s.value).map { v -> s.copy(value = v) } }
        .distinctBy { it.value.lowercase() }
        .sortedByDescending { it.value.length }
        .takeIf { it.isNotEmpty() }
        ?.joinToString("|") { s ->
            val q = Pattern.quote(s.value)
            if (s.wholeWord) "(?<![\\p{L}\\p{N}])$q(?![\\p{L}\\p{N}])" else q
        }
        ?.let { Pattern.compile(it, Pattern.CASE_INSENSITIVE or Pattern.UNICODE_CASE) }

    private val labels: Map<String, String> = buildMap {
        for (s in secrets) for (v in variants(s.value)) putIfAbsent(v.lowercase(), s.label)
    }

    fun scrub(text: String): String {
        if (text.isEmpty()) return text
        var out = URL.matcher(text).replaceAll(URL_MARK)
        out = AUTH_PARAM.matcher(out).replaceAll("$1$HIDDEN")
        val p = secretPattern ?: return out
        val m = p.matcher(out)
        if (!m.find()) return out
        val sb = StringBuffer()
        do {
            val label = labels[m.group().lowercase()] ?: SERVER
            m.appendReplacement(sb, Matcher.quoteReplacement(label))
        } while (m.find())
        m.appendTail(sb)
        return HOST_ADDRESS.matcher(sb).replaceAll(SERVER)
    }

    /** This scrubber and [other]'s values together (an earlier session's, the sign-in form's). */
    operator fun plus(other: Scrubber): Scrubber =
        if (other.secrets.isEmpty()) this else Scrubber((secrets + other.secrets).distinct())

    val isEmpty: Boolean get() = secrets.isEmpty()

    companion object {
        const val URL_MARK = "<url>"
        const val HIDDEN = "<hidden>"
        const val TOKEN = "<token>"
        const val SALT = "<salt>"
        const val USER = "<user>"
        const val SERVER = "<server>"

        /** Removes URLs and auth parameters, knowing no account yet. */
        val NONE = Scrubber(emptyList())

        // scheme://… or scheme%3A%2F%2F…, up to whitespace, a quote or a bracket.
        private val URL: Pattern = Pattern.compile(
            "(?i)\\b(?:https?|wss?)(?::|%3A)(?://|%2F%2F)[^\\s\"'<>()\\[\\]{}]+",
        )

        // u= t= s= p= after ? & ; or their encoded forms; the value runs to the next separator.
        private val AUTH_PARAM: Pattern = Pattern.compile(
            "(?i)((?:[?&;]|%3F|%26)(?:u|t|s|p)(?:=|%3D))(?:(?!%26)[^&\\s\"'<>#])*",
        )

        // "<server>/203.0.113.5:443", "<server>/[2001:db8::1]:443": the host's address and port, as
        // InetSocketAddress writes them.
        private val HOST_ADDRESS: Pattern = Pattern.compile(
            Pattern.quote(SERVER) + "/(?:\\[[0-9a-fA-F:.%]+\\]|\\d{1,3}(?:\\.\\d{1,3}){3}|[0-9a-fA-F]*:[0-9a-fA-F:.%]+)(?::\\d+)?",
        )

        /**
         * The values to hide for one account. [url] gives the host (and the domain above it, which
         * names the owner as surely); blank values are skipped.
         */
        fun of(url: String? = null, user: String? = null, salt: String? = null, token: String? = null): Scrubber {
            val list = mutableListOf<Secret>()
            token?.trim()?.takeIf { it.length >= 4 }?.let { list += Secret(it, TOKEN, wholeWord = false) }
            salt?.trim()?.takeIf { it.length >= 4 }?.let { list += Secret(it, SALT, wholeWord = false) }
            user?.trim()?.takeIf { it.isNotEmpty() }?.let { list += Secret(it, USER, wholeWord = it.length < 3) }
            url?.let(ServerAddress::host)?.let { host ->
                list += Secret(host, SERVER, wholeWord = false)
                domainOf(host)?.let { list += Secret(it, SERVER, wholeWord = false) }
            }
            return Scrubber(list)
        }

        /** `music.example.com` -> `example.com`; nothing for a two-label host or an IP address. */
        private fun domainOf(host: String): String? {
            if (':' in host || host.all { it.isDigit() || it == '.' }) return null
            val labels = host.split('.')
            if (labels.size < 3) return null
            return labels.drop(1).joinToString(".")
        }

        /** The forms a value turns up in: as is, percent-encoded (+ or %20), IDN both ways. */
        private fun variants(value: String): Set<String> {
            val forms = buildSet {
                add(value)
                runCatching { add(IDN.toASCII(value)) }
                runCatching { add(IDN.toUnicode(value)) }
            }
            return buildSet {
                for (f in forms) {
                    add(f)
                    // The String charset overloads: the Charset ones are Java 10, on Android only from API 33
                    // (minSdk is 31), and crashed Android 12 with NoSuchMethodError.
                    val encoded = URLEncoder.encode(f, "UTF-8")
                    add(encoded)
                    add(encoded.replace("+", "%20"))
                }
            }.filter { it.isNotEmpty() }.toSet()
        }
    }
}
