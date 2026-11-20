package com.offbeat.marquee.core.library

import com.offbeat.marquee.core.subsonic.Album
import com.offbeat.marquee.core.subsonic.AlbumListType
import java.text.Normalizer

/**
 * The Library's album sorts, as `getAlbumList2` types. Every sort but [Random] pages by offset in a
 * fixed order, so the grid can jump to any page; [Random] answers a fresh random page each time.
 */
enum class AlbumSort(val title: String, val type: AlbumListType) {
    RecentlyAdded("Recently added", AlbumListType.Newest),
    Name("Name", AlbumListType.AlphabeticalByName),
    Artist("Artist", AlbumListType.AlphabeticalByArtist),
    Year("Year", AlbumListType.ByYear),
    MostPlayed("Most played", AlbumListType.Frequent),
    Random("Random", AlbumListType.Random),
    ;

    /** A stable name for saved settings and the disk cache. */
    val key: String get() = name.lowercase()

    /** Pages by offset in a stable order: the grid knows its size and the fast scroller can jump. */
    val pagesByOffset: Boolean get() = this != Random

    /** The fast scroller's bubble: a letter on the name and artist sorts, the year on the year sort. */
    val bubble: Boolean get() = this == Name || this == Artist || this == Year

    /** A page, as `getAlbumList2` takes it. Year: newest first (fromYear > toYear), undated last. */
    fun query(size: Int, offset: Int, currentYear: Int): AlbumListQuery = when (this) {
        Year -> AlbumListQuery(type, size, offset, fromYear = currentYear + 1, toYear = 0)
        else -> AlbumListQuery(type, size, offset)
    }

    /** What the bubble shows for an album under this sort; null when the sort has no bubble. */
    fun bubbleText(album: Album, articles: List<String>): String? = when (this) {
        Name -> SortLetters.letterOf(album.name, articles)
        Artist -> SortLetters.letterOf(album.artist.orEmpty(), articles)
        Year -> album.year?.takeIf { it > 0 }?.toString() ?: "—"
        else -> null
    }

    /**
     * The same order on a list held locally (the Favourites chip filters to starred albums, which
     * come in one answer). Random shuffles with [seed] so a recomposition keeps the order.
     */
    fun sortLocally(albums: List<Album>, articles: List<String>, seed: Long = 0): List<Album> = when (this) {
        RecentlyAdded -> albums.sortedByDescending { it.created.orEmpty() }
        Name -> albums.sortedWith(compareBy<Album> { SortLetters.sortKey(it.name, articles) }.thenBy { it.year ?: 0 })
        Artist -> albums.sortedWith(
            compareBy<Album> { SortLetters.sortKey(it.artist.orEmpty(), articles) }
                .thenBy { it.year ?: 0 }
                .thenBy { SortLetters.sortKey(it.name, articles) },
        )
        Year -> albums.sortedWith(compareByDescending<Album> { it.year ?: 0 }.thenBy { SortLetters.sortKey(it.name, articles) })
        MostPlayed -> albums.sortedByDescending { it.playCount ?: 0 }
        Random -> albums.shuffled(kotlin.random.Random(seed))
    }

    companion object {
        val DEFAULT = RecentlyAdded
        fun fromKey(key: String?): AlbumSort = entries.firstOrNull { it.key == key } ?: DEFAULT
    }
}

/**
 * Letters for scrollers, the way Navidrome orders names: a leading article is ignored ("The Wall"
 * sorts under W, as the server's `ignoredArticles` say), accents are folded, and digits and
 * symbols go under "#".
 */
object SortLetters {
    /** Navidrome's default `ignoredArticles`, used until the server's own list is known. */
    val DEFAULT_ARTICLES: List<String> = "The El La Los Las Le Les Os As O A".split(' ')

    fun articles(ignoredArticles: String?): List<String> =
        ignoredArticles?.split(' ')?.map { it.trim() }?.filter { it.isNotEmpty() }?.takeIf { it.isNotEmpty() } ?: DEFAULT_ARTICLES

    /** The name lowercased, accents folded, without a leading article. */
    fun sortKey(name: String, articles: List<String>): String {
        val folded = fold(name.trim()).lowercase()
        for (a in articles) {
            val prefix = a.lowercase() + " "
            if (folded.startsWith(prefix) && folded.length > prefix.length) return folded.substring(prefix.length).trimStart()
        }
        return folded
    }

    /** "A" to "Z", or "#" for anything else. */
    fun letterOf(name: String, articles: List<String>): String {
        val c = sortKey(name, articles).firstOrNull { !it.isWhitespace() } ?: return "#"
        val u = c.uppercaseChar()
        return if (u in 'A'..'Z') u.toString() else "#"
    }

    private val marks = Regex("\\p{Mn}+")

    private fun fold(s: String): String = marks.replace(Normalizer.normalize(s, Normalizer.Form.NFD), "")
}
