package com.offbeat.marquee.core.offline

import com.offbeat.marquee.core.subsonic.Song
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PlaylistShrinkTest {
    private fun songs(vararg ids: String) = ids.map { Song(it, title = it) }
    private val twelve = (1..12).map { "s$it" }.toTypedArray()
    private val pinned = PinSet().with(Pin(PinKind.Playlist, "pl", songs = songs(*twelve))).pins

    @Test
    fun anEmptyAnswerIsHeldThenFollowedWhenALaterOneSaysTheSame() {
        // Before: the first empty answer released all twelve songs (their downloads deleted).
        val first = pinned.followPlaylist("pl", emptyList(), null, nowMs = 1_000)
        assertTrue(first.held)
        assertTrue(first.release.isEmpty())
        assertEquals(12, first.pins.wanted().size)

        // Seconds later (the page opened): still held.
        val soon = first.pins.followPlaylist("pl", emptyList(), null, nowMs = 61_000)
        assertTrue(soon.held)

        // A later sync says the same: followed.
        val later = soon.pins.followPlaylist("pl", emptyList(), null, nowMs = 1_000 + PlaylistShrink.CONFIRM_AFTER_MS)
        assertFalse(later.held)
        assertEquals(twelve.toSet(), later.release)
        assertEquals(null, later.pins["playlist:pl"]!!.shrinkSeen)
    }

    @Test
    fun aGlitchThatPassesChangesNothing() {
        val held = pinned.followPlaylist("pl", songs("s1"), null, nowMs = 0)
        assertTrue(held.held)
        val back = held.pins.followPlaylist("pl", songs(*twelve), null, nowMs = 3_600_000)
        assertFalse(back.held)
        assertTrue(back.release.isEmpty())
        assertEquals(null, back.pins["playlist:pl"]!!.shrinkSeen)
    }

    @Test
    fun aDifferentSuspectAnswerStartsTheWaitAgain() {
        val a = pinned.followPlaylist("pl", emptyList(), null, nowMs = 0)
        val b = a.pins.followPlaylist("pl", songs("s1"), null, nowMs = PlaylistShrink.CONFIRM_AFTER_MS)
        assertTrue(b.held)
    }

    @Test
    fun ordinaryEditsAreFollowedAtOnce() {
        val c = pinned.followPlaylist("pl", songs(*twelve.drop(3).toTypedArray()), null, nowMs = 0)
        assertFalse(c.held)
        assertEquals(setOf("s1", "s2", "s3"), c.release)
        // A small playlist emptied is still suspect; one shrinking by half is not.
        assertTrue(PlaylistShrink.isSuspect(2, 0))
        assertFalse(PlaylistShrink.isSuspect(4, 2))
        assertFalse(PlaylistShrink.isSuspect(12, 3))
        assertTrue(PlaylistShrink.isSuspect(12, 2))
    }
}
