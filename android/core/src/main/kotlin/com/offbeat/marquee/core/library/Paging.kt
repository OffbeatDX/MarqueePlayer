package com.offbeat.marquee.core.library

/**
 * The Albums grid's paging. The grid is as long as the library from the start (cells not fetched
 * yet draw as placeholders), and only the pages near what is on screen are fetched, so the fast
 * scroller can jump anywhere without loading what lies between.
 */
object AlbumPaging {
    /** Albums per request. */
    const val PAGE_SIZE = 60

    /** Albums fetched ahead of (and behind) what is on screen. */
    const val PREFETCH = 30

    fun pageOf(index: Int, pageSize: Int = PAGE_SIZE): Int = index.coerceAtLeast(0) / pageSize

    fun offsetOf(page: Int, pageSize: Int = PAGE_SIZE): Int = page * pageSize

    fun pageCount(total: Int, pageSize: Int = PAGE_SIZE): Int = (total + pageSize - 1) / pageSize

    /**
     * The pages wanted while cells [first]..[last] are on screen: those holding them and the
     * [prefetch] cells either side, within [total].
     */
    fun wantedPages(first: Int, last: Int, total: Int, pageSize: Int = PAGE_SIZE, prefetch: Int = PREFETCH): IntRange {
        if (total <= 0 || last < 0) return IntRange.EMPTY
        val lo = (first - prefetch).coerceIn(0, total - 1)
        val hi = (last + prefetch).coerceIn(0, total - 1)
        return pageOf(lo, pageSize)..pageOf(hi, pageSize)
    }

    /** The cell the fast scroller's thumb points at, [fraction] of the way down. */
    fun indexAt(fraction: Float, total: Int): Int =
        if (total <= 0) 0 else (fraction.coerceIn(0f, 1f) * (total - 1)).toInt()

    /** Where the thumb sits with [first] at the top and [visible] cells on screen. */
    fun fractionOf(first: Int, visible: Int, total: Int): Float {
        val scrollable = total - visible
        return if (scrollable <= 0) 0f else (first.toFloat() / scrollable).coerceIn(0f, 1f)
    }
}

/**
 * How many albums a list holds. Subsonic has no count for `getAlbumList2` (and `getScanStatus`
 * counts files), so it is found by asking for one album at an offset ([exists]). With last time's
 * count as [hint] two tiny requests confirm it; without one, a gallop then a binary search (about a
 * dozen requests for a thousand albums).
 */
object CountSearch {
    suspend fun count(hint: Int?, start: Int = 1024, exists: suspend (offset: Int) -> Boolean): Int {
        if (hint != null && hint >= 0) {
            val below = hint == 0 || exists(hint - 1)
            if (below && !exists(hint)) return hint
        }
        if (!exists(0)) return 0
        // Gallop: lo exists, hi does not.
        var lo = 0
        var hi = start.coerceAtLeast(1)
        while (exists(hi)) {
            lo = hi
            hi *= 2
        }
        // Binary search for the first offset that does not exist.
        while (hi - lo > 1) {
            val mid = lo + (hi - lo) / 2
            if (exists(mid)) lo = mid else hi = mid
        }
        return hi
    }
}
