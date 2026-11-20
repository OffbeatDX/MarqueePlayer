package com.offbeat.marquee.core.library

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ThrottleTest {
    @Test
    fun dueOnceThenNotUntilTheIntervalHasPassed() {
        val hour = Throttle.PLAYLIST_SYNC_MS
        assertTrue(Throttle.isDue(0, 5_000, hour))
        // A rotation a minute later: not again.
        assertFalse(Throttle.isDue(5_000, 65_000, hour))
        assertTrue(Throttle.isDue(5_000, 5_000 + hour, hour))
        // The clock set back: due rather than stuck.
        assertTrue(Throttle.isDue(5_000, 1_000, hour))
    }
}
