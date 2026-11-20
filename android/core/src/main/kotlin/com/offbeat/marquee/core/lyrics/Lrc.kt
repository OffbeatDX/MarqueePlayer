package com.offbeat.marquee.core.lyrics

/**
 * LRC, as LRCLIB's `syncedLyrics` (and the odd server tag) has it. A port of the desktop's
 * `library::lyrics::parse`:
 * - stamps `[mm:ss.xx]`, also `[mm:ss]`, `[mm:ss.xxx]`, `[mm:ss:xx]` and `[h:mm:ss.xx]`; several on one
 *   line (a repeated chorus) give one line each;
 * - `[offset:±ms]` is kept as the offset; other ID tags (`[ar:...]`, `[length:...]`) are ignored, while
 *   other brackets (`[Chorus: Someone]`) stay text;
 * - the enhanced format's word stamps (`<mm:ss.xx>`) and voice tags (`v1:`) are dropped from the text;
 * - blank stamped lines stay (an instrumental break), blank unstamped lines are dropped in synced text;
 * - text without any stamp is plain lyrics, kept as written without leading or trailing blank lines.
 */
object Lrc {
    /** The LRC ID tags; any other `[key: value]` is text, such as a section header. */
    private val ID_TAGS = setOf("ar", "al", "ti", "au", "lr", "by", "re", "ve", "length", "offset", "tool", "#")

    fun parse(text: String, source: LyricsSource): Lyrics {
        val timed = ArrayList<Pair<Long, String>>()
        val plain = ArrayList<String>()
        var offset = 0L
        for (raw in text.removePrefix("﻿").lines()) {
            var rest = raw.trim()
            val times = ArrayList<Long>(1)
            var tagLine = false
            while (rest.startsWith("[")) {
                val end = rest.indexOf(']')
                if (end < 0) break
                val body = rest.substring(1, end)
                val t = stamp(body)
                if (t != null) {
                    times += t
                } else {
                    val colon = body.indexOf(':')
                    if (colon < 0) break
                    val key = body.substring(0, colon).trim().lowercase()
                    // "[Verse 1: Someone]" is a section header, kept as text.
                    if (key !in ID_TAGS) break
                    if (key == "offset") {
                        offset = body.substring(colon + 1).trim().removePrefix("+").toLongOrNull() ?: 0
                    }
                    tagLine = true
                }
                rest = rest.substring(end + 1).trimStart()
            }
            if (times.isEmpty()) {
                if (!tagLine || rest.isNotEmpty()) plain += rest
                continue
            }
            val clean = clean(rest)
            for (t in times) timed += t to clean
        }
        if (timed.isEmpty()) {
            val lines = plain.dropWhile { it.isBlank() }.dropLastWhile { it.isBlank() }
            return Lyrics(synced = false, lines = lines.map { LyricLine(null, it) }, source = source)
        }
        // Stable: lines with the same stamp keep their order.
        timed.sortBy { it.first }
        return Lyrics(synced = true, lines = timed.map { LyricLine(it.first, it.second) }, source = source, offsetMs = offset)
    }

    /** `mm:ss.xx` and its variants, in ms; null when [s] is not a time stamp. */
    internal fun stamp(s: String): Long? {
        val parts = s.split(':')
        fun num(x: String): Long? = x.trim().takeIf { it.isNotEmpty() && it.all(Char::isDigit) }?.toLongOrNull()
        fun secMs(x: String): Long? {
            val dot = x.indexOf('.')
            val sec = num(if (dot < 0) x else x.substring(0, dot)) ?: return null
            val frac = if (dot < 0) "" else x.substring(dot + 1).trim()
            val ms = if (frac.isEmpty()) {
                0
            } else {
                val digits = frac.take(3)
                (num(digits) ?: return null) * pow10(3 - digits.length)
            }
            return sec * 1000 + ms
        }
        return when (parts.size) {
            2 -> {
                val m = num(parts[0]) ?: return null
                val ms = secMs(parts[1]) ?: return null
                m * 60_000 + ms
            }
            // h:mm:ss.xx, or mm:ss:xx (centiseconds after a colon)
            3 -> if (parts[2].contains('.')) {
                val h = num(parts[0]) ?: return null
                val m = num(parts[1]) ?: return null
                val ms = secMs(parts[2]) ?: return null
                h * 3_600_000 + m * 60_000 + ms
            } else {
                val m = num(parts[0]) ?: return null
                val sec = num(parts[1]) ?: return null
                val cs = num(parts[2]) ?: return null
                m * 60_000 + sec * 1000 + cs * 10
            }
            else -> null
        }
    }

    private fun pow10(n: Int): Long {
        var r = 1L
        repeat(n) { r *= 10 }
        return r
    }

    /** The enhanced LRC's word stamps and voice tags out of a line, runs of spaces made one. */
    internal fun clean(s: String): String {
        val out = StringBuilder(s.length)
        var rest = s
        while (true) {
            val i = rest.indexOf('<')
            if (i < 0) break
            out.append(rest, 0, i)
            val j = rest.indexOf('>', i)
            if (j > i && stamp(rest.substring(i + 1, j)) != null) {
                rest = rest.substring(j + 1)
            } else {
                out.append('<')
                rest = rest.substring(i + 1)
            }
        }
        out.append(rest)
        val joined = out.trim().split(Regex("\\s+")).filter { it.isNotEmpty() }.joinToString(" ")
        // "v1: words" (voice / singer tags of the A2 extension).
        val colon = joined.indexOf(':')
        if (colon in 2..3 && joined[0] == 'v' && joined.substring(1, colon).all(Char::isDigit)) {
            return joined.substring(colon + 1).trim()
        }
        return joined
    }

    /** Lyrics back to text (LRC when synced), which [parse] reads back as they were. The offset is written as a tag. */
    fun format(lyrics: Lyrics): String = buildString {
        if (lyrics.synced && lyrics.offsetMs != 0L) append("[offset:").append(lyrics.offsetMs).append("]\n")
        lyrics.lines.forEachIndexed { i, line ->
            if (i > 0) append('\n')
            val ms = line.startMs
            if (lyrics.synced && ms != null) {
                append('[')
                append((ms / 60_000).toString().padStart(2, '0')).append(':')
                append(((ms / 1000) % 60).toString().padStart(2, '0')).append('.')
                append((ms % 1000).toString().padStart(3, '0')).append(']')
            }
            append(line.text)
        }
    }
}
