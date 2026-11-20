package com.offbeat.marquee.core.offline

/**
 * What can play without a network: a song that is downloaded or whole in the cache. With a
 * network anything can.
 */
object Playable {
    fun isPlayable(online: Boolean, downloaded: Boolean, fullyCached: Boolean): Boolean = online || downloaded || fullyCached

    /**
     * "Play" on a list offline: the songs that can play, starting from the first playable one at
     * or after [start] (or the last one before it when none is after). Null when nothing can play.
     */
    fun <T> queueFrom(songs: List<T>, start: Int, playable: (T) -> Boolean): Pair<List<T>, Int>? {
        if (songs.isEmpty()) return null
        val keep = songs.indices.filter { playable(songs[it]) }
        if (keep.isEmpty()) return null
        val from = start.coerceIn(0, songs.lastIndex)
        val first = keep.firstOrNull { it >= from } ?: keep.last()
        return keep.map { songs[it] } to keep.indexOf(first)
    }

    /**
     * The next playable entry after [current] in play [order] (timeline indices), wrapping when
     * [wrap] (repeat all). Null when none.
     */
    fun nextIn(order: List<Int>, current: Int, wrap: Boolean, playable: (Int) -> Boolean): Int? {
        val at = order.indexOf(current)
        if (at < 0) return order.firstOrNull(playable)
        val after = order.subList(at + 1, order.size)
        after.firstOrNull(playable)?.let { return it }
        return if (wrap) order.subList(0, at).firstOrNull(playable) else null
    }
}
