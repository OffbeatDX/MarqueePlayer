package com.offbeat.marquee.core.playback

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ResumeTest {
    @Test
    fun aPartIsContinuedOnlyWithAValidator() {
        assertEquals(Resume.Plan.Start, Resume.plan(0, null))
        assertEquals(Resume.Plan.Start, Resume.plan(0, "\"abc\""))
        assertEquals(Resume.Plan.Restart, Resume.plan(1_000, null))
        assertEquals(Resume.Plan.Restart, Resume.plan(1_000, " "))
        assertEquals(Resume.Plan.Continue("\"abc\""), Resume.plan(1_000, "\"abc\""))
    }

    @Test
    fun theValidatorIsAStrongEtagElseLastModified() {
        assertEquals("\"v1\"", Resume.validatorOf("\"v1\"", "Wed, 18 Nov 2026 10:00:00 GMT"))
        assertEquals("Wed, 18 Nov 2026 10:00:00 GMT", Resume.validatorOf("W/\"v1\"", "Wed, 18 Nov 2026 10:00:00 GMT"))
        assertEquals("Wed, 18 Nov 2026 10:00:00 GMT", Resume.validatorOf(null, "Wed, 18 Nov 2026 10:00:00 GMT"))
        assertNull(Resume.validatorOf(null, null))
        assertNull(Resume.validatorOf("W/\"weak\"", null))
    }

    @Test
    fun readsTheRangeStart() {
        assertEquals(1234L, Resume.rangeStart("bytes=1234-"))
        assertEquals(10L, Resume.rangeStart("bytes=10-99"))
        assertEquals(0L, Resume.rangeStart(null))
        assertEquals(0L, Resume.rangeStart("items=1-"))
    }

    @Test
    fun aWholeFileOrAnotherValidatorMeansTheFileChanged() {
        // Before: the 200 was skipped forward and its tail stored after the old part.
        assertTrue(Resume.isChanged(1_000, "\"v1\"", 200, "\"v2\""))
        assertTrue(Resume.isChanged(1_000, "\"v1\"", 206, "\"v2\""))
        assertFalse(Resume.isChanged(1_000, "\"v1\"", 206, "\"v1\""))
        assertFalse(Resume.isChanged(1_000, "\"v1\"", 206, null))
        // Not a resume: a live transcode answering 200 to a plain range is the player's business.
        assertFalse(Resume.isChanged(1_000, null, 200, null))
        assertFalse(Resume.isChanged(0, "\"v1\"", 200, "\"v2\""))
    }
}
