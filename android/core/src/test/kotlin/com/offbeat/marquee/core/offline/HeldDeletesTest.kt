package com.offbeat.marquee.core.offline

import org.junit.Assert.assertEquals
import org.junit.Test

class HeldDeletesTest {
    @Test
    fun thePlayingAndNextSongsWaitUntilTheyLeaveThePlayWindow() {
        val h = HeldDeletes()
        h.setInUse(setOf("now", "next")) { false }
        // Before: all three were deleted at once, the playing one under the player.
        assertEquals(setOf("other"), h.release(setOf("now", "next", "other")))
        assertEquals(setOf("now", "next"), h.heldIds)
        // The track ends: "now" leaves, "next" plays, "after" is next.
        assertEquals(setOf("now"), h.setInUse(setOf("next", "after")) { false })
        assertEquals(setOf("next"), h.setInUse(emptySet()) { false })
        assertEquals(emptySet<String>(), h.heldIds)
    }

    @Test
    fun aSongPinnedAgainMeanwhileStays() {
        val h = HeldDeletes()
        h.setInUse(setOf("now")) { false }
        assertEquals(emptySet<String>(), h.release(setOf("now")))
        assertEquals(emptySet<String>(), h.setInUse(emptySet()) { it == "now" })
        assertEquals(emptySet<String>(), h.heldIds)
    }

    @Test
    fun nothingPlayingNothingHeld() {
        assertEquals(setOf("a", "b"), HeldDeletes().release(setOf("a", "b")))
    }
}
