package com.offbeat.marquee.core.playback

import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** The timer's delays follow uptime; deep sleep moves only the real clock. */
class SleepTimerDeepSleepTest {
    private class Rig(scope: TestScope) {
        var slept = 0L
        var expired = 0
        val fades = mutableListOf<Float>()
        val timer = SleepTimer(
            scope = scope.backgroundScope,
            clock = { scope.testScheduler.currentTime + slept },
            onFade = { fades += it },
            onExpire = { expired++ },
        )
    }

    @Test
    fun aTimerThatEndedInDeepSleepIsCancelledNotPausedLater() = runTest {
        val r = Rig(this)
        r.timer.setMinutes(10)
        advanceTimeBy(60_000)
        // Paused, screen off: 20 minutes of deep sleep, during which no delay runs.
        r.slept += 20 * 60_000L
        assertTrue(r.timer.recheck())
        assertEquals(SleepMode.Off, r.timer.mode.value)
        // Before: the old delay still fired 9 minutes of uptime later and paused with no fade.
        advanceTimeBy(30 * 60_000L)
        assertEquals(0, r.expired)
    }

    @Test
    fun aTimerStillToComeIsArmedForTheTimeReallyLeft() = runTest {
        val r = Rig(this)
        r.timer.setMinutes(10)
        advanceTimeBy(60_000)
        r.slept += 5 * 60_000L // 6 of the 10 minutes gone in real time
        assertFalse(r.timer.recheck())
        // It fades and pauses 4 real minutes on, not 9.
        advanceTimeBy(4 * 60_000L - SleepTimer.FADE_MS - 1)
        assertTrue(r.fades.none { it < 1f })
        advanceTimeBy(SleepTimer.FADE_MS + 1)
        runCurrent()
        assertEquals(1, r.expired)
        assertTrue(r.fades.any { it in 0.01f..0.99f })
        assertEquals(SleepMode.Off, r.timer.mode.value)
    }

    @Test
    fun recheckLeavesOtherModesAlone() = runTest {
        val r = Rig(this)
        assertFalse(r.timer.recheck())
        r.timer.setEndOfTrack()
        assertFalse(r.timer.recheck())
        assertEquals(SleepMode.EndOfTrack, r.timer.mode.value)
    }
}
