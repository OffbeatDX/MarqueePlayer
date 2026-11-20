package com.offbeat.marquee.core.lyrics

import kotlinx.serialization.Serializable

/**
 * One entry of OpenSubsonic's `getLyricsBySongId` (`lyricsList.structuredLyrics[]`). Navidrome
 * returns one per lyrics tag or `.lrc` file it found: synced and unsynced, maybe in several
 * languages.
 */
@Serializable
data class StructuredLyrics(
    val lang: String? = null,
    val synced: Boolean = false,
    val line: List<StructuredLine> = emptyList(),
    val offset: Long? = null,
    val displayArtist: String? = null,
    val displayTitle: String? = null,
)

@Serializable
data class StructuredLine(val start: Long? = null, val value: String = "")

/** Turning the server's entries into [Lyrics], and choosing between several. */
object ServerLyrics {
    /**
     * The entry to show: synced first, then one in a language the device uses ([deviceLanguages]:
     * two- or three-letter codes, most preferred first), then the first. Entries without words
     * are skipped. Null when nothing has words.
     */
    fun choose(entries: List<StructuredLyrics>, deviceLanguages: List<String>): Lyrics? {
        val usable = entries.mapNotNull { e -> toLyrics(e)?.let { e to it } }
        if (usable.isEmpty()) return null
        val langs = deviceLanguages.map { it.lowercase() }
        fun langRank(lang: String?): Int {
            val l = lang?.lowercase()?.substringBefore('-')?.substringBefore('_') ?: return langs.size
            val i = langs.indexOfFirst { it == l || LanguageCodes.same(it, l) }
            return if (i < 0) langs.size else i
        }
        // sortedWith is stable, so equal entries keep the server's order (the first wins).
        return usable.sortedWith(compareBy({ if (it.second.synced) 0 else 1 }, { langRank(it.first.lang) })).first().second
    }

    /**
     * One entry as [Lyrics]. A synced entry needs a start on its lines; lines without one are
     * dropped. An unsynced entry whose text is really LRC (a tag with stamps that the server did
     * not parse) is read as LRC. Null when there are no words.
     */
    fun toLyrics(e: StructuredLyrics): Lyrics? {
        val lang = e.lang?.takeIf { it.isNotBlank() && !it.equals("xxx", true) && !it.equals("und", true) }
        val offset = e.offset ?: 0
        val l = if (e.synced) {
            val lines = e.line.filter { it.start != null }.sortedBy { it.start }.map { LyricLine(it.start, Lrc.clean(it.value)) }
            if (lines.isEmpty()) return null
            Lyrics(synced = true, lines = lines, source = LyricsSource.Server, offsetMs = offset, lang = lang)
        } else {
            val text = e.line.joinToString("\n") { it.value }
            val parsed = Lrc.parse(text, LyricsSource.Server)
            if (parsed.synced) parsed.copy(offsetMs = parsed.offsetMs + offset, lang = lang) else parsed.copy(lang = lang)
        }
        return l.takeUnless { it.isEmpty }
    }
}

/** ISO 639-1 (device locales) against 639-2 (what tags often carry) for the common languages. */
internal object LanguageCodes {
    private val pairs = mapOf(
        "en" to listOf("eng"), "fr" to listOf("fra", "fre"), "de" to listOf("deu", "ger"), "es" to listOf("spa"),
        "it" to listOf("ita"), "pt" to listOf("por"), "nl" to listOf("nld", "dut"), "ja" to listOf("jpn"),
        "ko" to listOf("kor"), "zh" to listOf("zho", "chi"), "ru" to listOf("rus"), "sv" to listOf("swe"),
        "no" to listOf("nor", "nob", "nno"), "da" to listOf("dan"), "fi" to listOf("fin"), "pl" to listOf("pol"),
        "tr" to listOf("tur"), "ar" to listOf("ara"), "he" to listOf("heb"), "hi" to listOf("hin"), "el" to listOf("ell", "gre"),
        "cs" to listOf("ces", "cze"), "hu" to listOf("hun"), "uk" to listOf("ukr"), "ca" to listOf("cat"),
    )

    fun same(a: String, b: String): Boolean {
        if (a == b) return true
        return pairs[a]?.contains(b) == true || pairs[b]?.contains(a) == true
    }
}
