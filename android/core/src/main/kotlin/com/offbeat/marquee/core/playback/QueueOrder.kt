package com.offbeat.marquee.core.playback

/**
 * Queue edits as play-order maths. The player keeps its items in one list (the timeline) and, with
 * shuffle on, a play order over it (a permutation of the timeline indices). The queue sheet shows
 * play order, so edits are said in play positions; the service turns them into timeline moves
 * (shuffle off) or a new play order (shuffle on).
 */
object QueueOrder {
    /** The play order after [count] items were inserted into the timeline at [at]: later indices shift. */
    private fun shifted(order: List<Int>, at: Int, count: Int): List<Int> = order.map { if (it >= at) it + count else it }

    /** "Play next" inserts into the timeline right after [current], so the items are next with shuffle off too. */
    fun timelineIndexForPlayNext(current: Int, size: Int): Int = if (size == 0) 0 else (current + 1).coerceIn(0, size)

    /** The play order after "Play next": the new items right after [current], in their own order. */
    fun orderAfterPlayNext(order: List<Int>, current: Int, count: Int): List<Int> {
        val at = timelineIndexForPlayNext(current, order.size)
        val base = shifted(order, at, count)
        val pos = order.indexOf(current)
        val inserted = (at until at + count).toList()
        return if (pos < 0) inserted + base else base.subList(0, pos + 1) + inserted + base.subList(pos + 1, base.size)
    }

    /** The play order after "Add to queue": the new items at the end of the timeline and of the play order. */
    fun orderAfterAppend(order: List<Int>, count: Int): List<Int> = order + (order.size until order.size + count)

    /** Moves the entry at play position [from] to position [to]. */
    fun move(order: List<Int>, from: Int, to: Int): List<Int> {
        if (from !in order.indices || to !in order.indices || from == to) return order
        val out = order.toMutableList()
        val v = out.removeAt(from)
        out.add(to, v)
        return out
    }

    /**
     * Where a row dragged in the queue sheet lands, as a [move] (from, to) in play positions.
     * [queue] is the queue's entry ids in play order now and [currentPos] the current track's
     * position in it; [arranged] is the upcoming ids as the drag has arranged them, which can come
     * from an older snapshot (the queue may change during a drag). The row goes right after the
     * nearest row above it in [arranged] that is still upcoming, or right after the current track.
     * Null when there is nothing to move: [uid] has left the queue, is no longer upcoming, or is
     * already there.
     */
    fun dropMove(queue: List<Long>, currentPos: Int, arranged: List<Long>, uid: Long): Pair<Int, Int>? {
        val from = queue.indexOf(uid)
        val at = arranged.indexOf(uid)
        if (from <= currentPos || at < 0) return null
        val after = arranged.subList(0, at).asReversed().map { queue.indexOf(it) }.firstOrNull { it > currentPos } ?: currentPos
        // Taking the row out first shifts a later "after" back by one.
        val to = if (after < from) after + 1 else after
        return if (to == from) null else from to to
    }

    /** The timeline indices after [current] in play order (what "Clear upcoming" removes). */
    fun upcoming(order: List<Int>, current: Int): List<Int> {
        val pos = order.indexOf(current)
        return if (pos < 0) emptyList() else order.subList(pos + 1, order.size)
    }

    /** The play order once timeline item [removed] is gone: the others keep their order. */
    fun orderAfterRemove(order: List<Int>, removed: Int): List<Int> =
        order.filter { it != removed }.map { if (it > removed) it - 1 else it }

    /** Whether [order] is a play order over [size] items (each index once). */
    fun isValid(order: List<Int>, size: Int): Boolean =
        order.size == size && order.all { it in 0 until size } && order.toSet().size == size

    fun identity(size: Int): List<Int> = (0 until size).toList()
}
