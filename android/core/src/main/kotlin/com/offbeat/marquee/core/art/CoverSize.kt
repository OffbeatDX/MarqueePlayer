package com.offbeat.marquee.core.art

/**
 * The few sizes covers are asked in (`getCoverArt?size=`), so the disk cache reuses them: each
 * place asks for the bucket that covers its drawn size.
 */
object CoverSize {
    /** Thumbnails: the mini player, the hero's strip, the queue. */
    const val THUMB = 150
    /** Grid cells and shelf rows. */
    const val GRID = 300
    /** Album and artist headers, the hero's featured cover. */
    const val HEADER = 600
    /** Now Playing. */
    const val FULL = 1000

    /** The tiny image the accent and the blurred backdrop are computed from. */
    const val SAMPLE = 64

    private val buckets = intArrayOf(THUMB, GRID, HEADER, FULL)

    /** The smallest bucket at least [px] (10% slack), else the largest. */
    fun bucketFor(px: Int): Int = buckets.firstOrNull { it * 1.1 >= px } ?: FULL
}
