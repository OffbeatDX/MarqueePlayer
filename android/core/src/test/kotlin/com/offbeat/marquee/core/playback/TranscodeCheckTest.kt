package com.offbeat.marquee.core.playback

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class TranscodeCheckTest {
    private val opus128 = StreamQuality.opus(128)

    @Test
    fun aTranscodeThatStoppedPartWayIsCutShort() {
        // A 4-minute song at Opus 128 is about 3.8 MB; the encoder died after 30 s.
        assertTrue(TranscodeCheck.isCutShort(opus128, 240, 480_000))
        assertTrue(TranscodeCheck.isCutShort(opus128, 240, 0))
    }

    @Test
    fun aWholeOneIsNot() {
        assertFalse(TranscodeCheck.isCutShort(opus128, 240, 3_800_000))
        // Quiet music well under the nominal rate.
        assertFalse(TranscodeCheck.isCutShort(opus128, 240, 1_700_000))
    }

    @Test
    fun nothingIsJudgedWithoutWhatItNeeds() {
        assertFalse(TranscodeCheck.isCutShort(StreamQuality.ORIGINAL, 240, 10))
        assertFalse(TranscodeCheck.isCutShort(opus128, null, 10))
        assertFalse(TranscodeCheck.isCutShort(opus128, 5, 10))
    }
}
