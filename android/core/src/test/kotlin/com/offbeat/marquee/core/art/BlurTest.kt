package com.offbeat.marquee.core.art

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class BlurTest {
    @Test
    fun `capBrightness brings bright pixels down to the cap and keeps their hue`() {
        val px = intArrayOf(0xFFFFFFFF.toInt(), 0xFFFF8000.toInt(), 0xFF203040.toInt())
        Blur.capBrightness(px, max = 150)
        assertEquals(0xFF969696.toInt(), px[0]) // white -> 150 grey
        assertEquals(0xFF964B00.toInt(), px[1]) // orange keeps its 2:1 red:green
        assertEquals(0xFF203040.toInt(), px[2]) // dark pixels are untouched
    }

    @Test
    fun `capBrightness keeps every channel at or under the default cap`() {
        val px = IntArray(256) { v -> (0xFF shl 24) or (v shl 16) or ((255 - v) shl 8) or 200 }
        Blur.capBrightness(px)
        for (p in px) {
            val top = maxOf((p shr 16) and 0xFF, (p shr 8) and 0xFF, p and 0xFF)
            assertTrue("channel $top over the cap", top <= Blur.BACKDROP_MAX_CHANNEL)
            assertEquals(0xFF, (p ushr 24) and 0xFF)
        }
    }
}
