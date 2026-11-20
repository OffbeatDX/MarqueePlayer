package com.offbeat.marquee.core.playback

import com.offbeat.marquee.core.subsonic.Song
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class QueueOrderTest {
    @Test
    fun playNextInOrderGoesRightAfterTheCurrentItem() {
        // Shuffle off: the order is the timeline. Current is 1 of [0,1,2,3].
        val order = QueueOrder.identity(4)
        assertEquals(2, QueueOrder.timelineIndexForPlayNext(1, 4))
        assertEquals(listOf(0, 1, 2, 3, 4, 5), QueueOrder.orderAfterPlayNext(order, current = 1, count = 2))
        assertEquals(0, QueueOrder.timelineIndexForPlayNext(0, 0))
        assertEquals(listOf(0, 1), QueueOrder.orderAfterPlayNext(emptyList(), current = 0, count = 2))
    }

    @Test
    fun playNextUnderShuffleIsNextInPlayOrder() {
        // Timeline of 5, play order 3,0,4,1,2; current is 4 (third played).
        val order = listOf(3, 0, 4, 1, 2)
        val out = QueueOrder.orderAfterPlayNext(order, current = 4, count = 2)
        // New items enter the timeline at 5 and 6, right after the current one.
        assertEquals(listOf(3, 0, 4, 5, 6, 1, 2), out)
        assertTrue(QueueOrder.isValid(out, 7))
        // Inserting mid-timeline shifts later indices: current 1 inserts at 2.
        val mid = QueueOrder.orderAfterPlayNext(listOf(3, 1, 0, 2), current = 1, count = 1)
        assertEquals(listOf(4, 1, 2, 0, 3), mid)
        assertTrue(QueueOrder.isValid(mid, 5))
    }

    @Test
    fun addToQueueGoesLast() {
        assertEquals(listOf(2, 0, 1, 3, 4), QueueOrder.orderAfterAppend(listOf(2, 0, 1), 2))
    }

    @Test
    fun movesAndUpcoming() {
        val order = listOf(3, 0, 4, 1, 2)
        assertEquals(listOf(3, 0, 4, 2, 1), QueueOrder.move(order, 4, 3))
        assertEquals(listOf(3, 4, 1, 0, 2), QueueOrder.move(order, 1, 3))
        assertEquals(order, QueueOrder.move(order, 1, 1))
        assertEquals(order, QueueOrder.move(order, 1, 9))
        assertEquals(listOf(1, 2), QueueOrder.upcoming(order, current = 4))
        assertEquals(emptyList<Int>(), QueueOrder.upcoming(order, current = 2))
        assertEquals(emptyList<Int>(), QueueOrder.upcoming(order, current = 7))
    }

    @Test
    fun removingKeepsTheRestInOrder() {
        assertEquals(listOf(2, 0, 3, 1), QueueOrder.orderAfterRemove(listOf(3, 0, 4, 1, 2), removed = 2))
    }

    @Test
    fun validity() {
        assertTrue(QueueOrder.isValid(listOf(1, 0, 2), 3))
        assertFalse(QueueOrder.isValid(listOf(1, 1, 2), 3))
        assertFalse(QueueOrder.isValid(listOf(0, 1), 3))
        assertFalse(QueueOrder.isValid(listOf(0, 1, 3), 3))
    }

    @Test
    fun savedQueueKeepsItsShuffleOrderOnlyWhenItFits() {
        val songs = listOf(Song("a"), Song("b"), Song("c"))
        val q = SavedQueue(songs, index = 1, positionMs = 5, shuffle = true, shuffleOrder = listOf(2, 0, 1))
        val back = SavedQueue.decode(q.encode())!!
        assertEquals(listOf(2, 0, 1), back.validShuffleOrder)
        assertNull(q.copy(shuffleOrder = listOf(0, 1)).validShuffleOrder)
        assertNull(SavedQueue.decode(q.copy(shuffleOrder = null).encode())!!.validShuffleOrder)
    }

    @Test
    fun aDraggedRowLandsAfterTheRowAboveItInTheLiveQueue() {
        // Queue A B C D E as uids 1..5, A playing.
        val q = listOf(1L, 2, 3, 4, 5)
        // D dragged to the top of Up next, to the bottom, or put back where it was.
        assertEquals(3 to 1, QueueOrder.dropMove(q, 0, listOf(4L, 2, 3, 5), 4))
        assertEquals(3 to 4, QueueOrder.dropMove(q, 0, listOf(2L, 3, 5, 4), 4))
        assertEquals(1 to 3, QueueOrder.dropMove(q, 0, listOf(3L, 4, 2, 5), 2))
        assertNull(QueueOrder.dropMove(q, 0, listOf(2L, 3, 4, 5), 4))
    }

    @Test
    fun aDropFollowsQueueChangesMadeSinceTheArrangementWasTaken() {
        // C was removed after the drag began: D (now position 2) dragged above B goes to position 1,
        // not whatever sits at D's old position 3.
        val removed = listOf(1L, 2, 4, 5)
        assertEquals(2 to 1, QueueOrder.dropMove(removed, 0, listOf(4L, 2, 3, 5), 4))
        // A row above it that has gone is skipped: D after C (gone) lands after B.
        assertNull(QueueOrder.dropMove(removed, 0, listOf(2L, 3, 4, 5), 4))
        assertEquals(3 to 2, QueueOrder.dropMove(removed, 0, listOf(2L, 3, 5, 4), 5))
        // The track changed to B: D dragged to the top lands right after B, and so does D under a
        // row that has since played.
        val q = listOf(1L, 2, 3, 4, 5)
        assertEquals(3 to 2, QueueOrder.dropMove(q, 1, listOf(4L, 2, 3, 5), 4))
        assertEquals(3 to 2, QueueOrder.dropMove(q, 1, listOf(2L, 4, 3, 5), 4))
        // The dragged row itself became current, or left the queue: nothing to move.
        assertNull(QueueOrder.dropMove(q, 3, listOf(4L, 5), 4))
        assertNull(QueueOrder.dropMove(listOf(1L, 2, 3), 0, listOf(2L, 4, 3), 4))
    }
}
