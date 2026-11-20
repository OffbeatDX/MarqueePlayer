package com.offbeat.marquee.core.playback

import com.offbeat.marquee.core.playback.SeekReload.Action
import com.offbeat.marquee.core.playback.SeekReload.Item
import com.offbeat.marquee.core.playback.SeekReload.Kind
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SeekReloadTest {
    private fun item(
        key: String = "a#1",
        prepared: Boolean = true,
        placeholder: Boolean = false,
        seekable: Boolean = false,
        stored: Boolean = false,
    ) = Item(key, prepared, placeholder, seekable, stored)

    @Test
    fun aSeekableTrackSeeksAsUsual() {
        val r = SeekReload()
        assertEquals(Action(Kind.Seek, 70_000), r.seek(item(seekable = true, stored = true), 70_000))
        assertEquals(Action(Kind.Seek, 70_000), r.seek(item(seekable = true), 70_000))
        assertNull(r.pendingMs)
    }

    @Test
    fun anUnpreparedEntryTakesThePositionToStartFrom() {
        assertEquals(Kind.Seek, SeekReload().seek(item(prepared = false, placeholder = true), 5_000).kind)
    }

    @Test
    fun anUnseekableTrackThatIsStoredIsReloadedAtThePosition() {
        val r = SeekReload()
        assertEquals(Action(Kind.Reload, 120_000), r.seek(item(stored = true), 120_000))
        assertNull(r.pendingMs)
    }

    @Test
    fun anUnseekableTrackNotStoredWaitsThenReloadsOnceStored() {
        val r = SeekReload()
        assertEquals(Action(Kind.WaitForTrack, 90_000), r.seek(item(), 90_000))
        assertEquals(90_000L, r.pendingMs)
        // Still fetching: nothing yet.
        assertNull(r.recheck(item()))
        // A newer seek replaces it.
        assertEquals(Kind.WaitForTrack, r.seek(item(), 150_000).kind)
        assertEquals(Action(Kind.Reload, 150_000), r.recheck(item(stored = true)))
        assertNull(r.pendingMs)
        // Done once only.
        assertNull(r.recheck(item(stored = true)))
    }

    @Test
    fun skippingElsewhereDropsThePendingSeek() {
        val r = SeekReload()
        r.seek(item(), 90_000)
        r.onOtherItem()
        assertNull(r.pendingMs)
        assertNull(r.recheck(item(stored = true)))
    }

    @Test
    fun aPendingSeekNeverLandsOnAnotherEntry() {
        val r = SeekReload()
        r.seek(item(key = "a#1"), 90_000)
        // The same song, another queue entry, now current and stored.
        assertNull(r.recheck(item(key = "a#2", stored = true)))
        assertNull(r.pendingMs)
    }

    @Test
    fun aSeekThePlayerCanDoReplacesThePendingOne() {
        val r = SeekReload()
        r.seek(item(), 90_000)
        assertEquals(Kind.Seek, r.seek(item(seekable = true), 10_000).kind)
        assertNull(r.pendingMs)
    }

    @Test
    fun backToTheStartNeedsNoWholeTrack() {
        val r = SeekReload()
        assertEquals(Action(Kind.Seek, 0), r.seek(item(), 0))
        assertEquals(Action(Kind.Seek, 0), r.seek(item(), -20))
        // Stored, it is reloaded so later seeks work at once.
        assertEquals(Action(Kind.Reload, 0), r.seek(item(stored = true), 0))
        // Asked while opening, then live and not stored: the restart needn't wait.
        val opening = SeekReload()
        opening.seek(item(placeholder = true), 0)
        assertEquals(Action(Kind.Seek, 0), opening.recheck(item()))
    }

    @Test
    fun whileOpeningTheSeekWaitsForTheSourceThenGoesWhereverItCan() {
        val r = SeekReload()
        assertEquals(Action(Kind.WaitForSource, 30_000), r.seek(item(placeholder = true), 30_000))
        assertNull(r.recheck(item(placeholder = true)))
        // It turned out seekable: an ordinary seek.
        assertEquals(Action(Kind.Seek, 30_000), r.recheck(item(seekable = true)))

        val live = SeekReload()
        live.seek(item(placeholder = true), 30_000)
        // A live transcode, not stored yet: keeps waiting for the track.
        assertNull(live.recheck(item()))
        assertEquals(30_000L, live.pendingMs)
        assertEquals(Action(Kind.Reload, 30_000), live.recheck(item(stored = true)))
    }

    @Test
    fun aSeekDuringTheReloadWaitsForItThenSeeks() {
        val r = SeekReload()
        assertEquals(Kind.Reload, r.seek(item(stored = true), 60_000).kind)
        // Reloading: the player still shows the old, unseekable timeline.
        assertEquals(Kind.WaitForSource, r.seek(item(stored = true), 100_000).kind)
        assertNull(r.recheck(item(stored = true, seekable = true)))
        assertFalse(r.onReady(item(stored = true, seekable = true)))
        assertEquals(Action(Kind.Seek, 100_000), r.recheck(item(stored = true, seekable = true)))
    }

    @Test
    fun aReloadThatStillCantSeekIsNotRepeated() {
        val r = SeekReload()
        assertEquals(Kind.Reload, r.seek(item(stored = true), 60_000).kind)
        assertTrue(r.onReady(item(stored = true)))
        assertEquals(Kind.Ignore, r.seek(item(stored = true), 90_000).kind)
        assertNull(r.pendingMs)
        // Another entry starts afresh.
        r.onOtherItem()
        assertEquals(Kind.Reload, r.seek(item(stored = true), 90_000).kind)
    }

    @Test
    fun readyWithoutAReloadChangesNothing() {
        val r = SeekReload()
        assertFalse(r.onReady(item()))
        assertEquals(Kind.WaitForTrack, r.seek(item(), 1_000).kind)
    }
}
