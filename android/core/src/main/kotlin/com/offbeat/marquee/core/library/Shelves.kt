package com.offbeat.marquee.core.library

import com.offbeat.marquee.core.subsonic.AlbumListType

/**
 * Home's shelves, as data: what each holds and how it is asked for (`getAlbumList2`), in the
 * desktop's default order (README "Home shelves"). Showing, hiding and reordering them
 * only changes the list handed to Home.
 */
enum class ShelfId(val title: String, val type: AlbumListType) {
    /** The hero card: its first album large, the next ones in a strip inside the card. */
    RecentlyAdded("Recently added", AlbumListType.Newest),
    RecentlyPlayed("Recently played", AlbumListType.Recent),
    /** Kept in its order until reshuffled. */
    Random("Random albums", AlbumListType.Random),
    MostPlayed("Most played", AlbumListType.Frequent),
    /** Newest release first: Navidrome sorts byYear descending when fromYear > toYear. */
    RecentlyReleased("Recently released", AlbumListType.ByYear),
    Favourites("Favourite albums", AlbumListType.Starred),
    ;

    val isHero: Boolean get() = this == RecentlyAdded
    val isRandom: Boolean get() = this == Random

    /** A stable name for the on-disk cache and saved state. */
    val key: String get() = name.lowercase()

    companion object {
        fun fromKey(key: String): ShelfId? = entries.firstOrNull { it.key == key }
    }
}

/** One page of a shelf's list, as `getAlbumList2` takes it. */
data class AlbumListQuery(
    val type: AlbumListType,
    val size: Int,
    val offset: Int,
    val fromYear: Int? = null,
    val toYear: Int? = null,
)

object Shelves {
    val DEFAULT: List<ShelfId> = listOf(
        ShelfId.RecentlyAdded, ShelfId.RecentlyPlayed, ShelfId.Random,
        ShelfId.MostPlayed, ShelfId.RecentlyReleased, ShelfId.Favourites,
    )

    /** Albums on a row (the desktop's page of 20). */
    const val ROW_SIZE = 20

    /** The hero's albums: the featured one and eleven in its strip. */
    const val HERO_SIZE = 12

    /** A page of a "See all" grid. */
    const val PAGE_SIZE = 60

    fun shelfSize(id: ShelfId): Int = if (id.isHero) HERO_SIZE else ROW_SIZE

    /**
     * The query for a page of [id]. Recently released asks byYear from next year down to 0, which
     * Navidrome answers newest first (by the latest year on the album, then name), undated last.
     */
    fun query(id: ShelfId, size: Int, offset: Int, currentYear: Int): AlbumListQuery = when (id) {
        ShelfId.RecentlyReleased -> AlbumListQuery(id.type, size, offset, fromYear = currentYear + 1, toYear = 0)
        else -> AlbumListQuery(id.type, size, offset)
    }
}
