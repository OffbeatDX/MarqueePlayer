package com.offbeat.marquee.core.art

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.math.abs

/** The desktop's accent tests (crates/library/src/accent.rs), ported, plus the app's own. */
class AccentTest {
    private val bg = 0x0C0B10

    private fun image(f: (Int, Int) -> Int): IntArray = IntArray(64 * 64) { i -> f(i % 64, i / 64) }

    private fun rgb(r: Int, g: Int, b: Int) = (r shl 16) or (g shl 8) or b

    private fun hue(c: Int) = Accent.toLch(Accent.oklab(c))[2]
    private fun chroma(c: Int) = Accent.toLch(Accent.oklab(c))[1]

    private fun assertUsable(c: Int) {
        val (accent, text) = Accent.usable(c)
        assertTrue("accent on bg: ${Accent.contrast(accent, bg)}", Accent.contrast(accent, bg) >= 7.0)
        assertTrue("text on accent: ${Accent.contrast(accent, text)}", Accent.contrast(accent, text) >= 7.0)
        assertTrue("accent is grey (chroma ${chroma(accent)})", chroma(accent) >= 0.05)
    }

    @Test
    fun oklabRoundTrips() {
        for (c in listOf(rgb(0, 0, 0), rgb(255, 255, 255), rgb(255, 179, 71), rgb(12, 200, 90), rgb(30, 60, 250))) {
            assertEquals(Integer.toHexString(c), c, Accent.oklabToRgb(Accent.oklab(c)))
        }
    }

    @Test
    fun vividPatchBeatsALargerDullBackground() {
        val red = rgb(220, 30, 40)
        val img = image { x, y -> if (x < 32 && y < 32) red else rgb(110, 100, 90) }
        val c = Accent.extract(img)!!
        assertTrue(Integer.toHexString(c), abs(hue(c) - hue(red)) < 0.2)
    }

    @Test
    fun almostBlackArtWithASmallLogoTakesTheLogo() {
        val img = image { x, y -> if (x in 28..35 && y in 28..35) rgb(40, 200, 230) else rgb(6, 6, 8) }
        val c = Accent.extract(img)!!
        assertTrue(Integer.toHexString(c), (c and 0xFF) > (c shr 16 and 0xFF))
        assertUsable(c)
    }

    @Test
    fun colourlessArtHasNoAccent() {
        assertNull(Accent.extract(image { _, _ -> rgb(3, 3, 3) }))
        assertNull(Accent.extract(image { _, _ -> rgb(250, 250, 248) }))
        assertNull(Accent.extract(image { x, y -> val v = (x + y) * 2; rgb(v, v, v) }))
        assertNull(Accent.extract(IntArray(0)))
    }

    @Test
    fun colourlessArtFallsBack() {
        assertEquals(Accent.FALLBACK, Accent.forPixels(image { x, _ -> rgb(x * 4, x * 4, x * 4) }))
    }

    @Test
    fun singleColourNeonIsTamed() {
        for (neon in listOf(rgb(255, 0, 255), rgb(0, 255, 0), rgb(0, 0, 255), rgb(255, 255, 0), rgb(255, 0, 0))) {
            val c = Accent.extract(image { _, _ -> neon })!!
            assertUsable(c)
            val ch = chroma(Accent.usable(c).first)
            assertTrue("${Integer.toHexString(neon)} -> chroma $ch", ch <= Accent.C_RANGE.endInclusive + 0.001)
        }
    }

    @Test
    fun darkAndPaleColoursAreLiftedOrLoweredToUsable() {
        for (c in listOf(rgb(40, 10, 60), rgb(10, 30, 15), rgb(250, 235, 240), rgb(200, 230, 255), rgb(120, 70, 20))) {
            assertUsable(c)
        }
    }

    @Test
    fun clampedLightnessAndChromaStayInRange() {
        for (c in listOf(rgb(255, 0, 0), rgb(20, 20, 120), rgb(240, 240, 200), rgb(90, 160, 90))) {
            val lch = Accent.toLch(Accent.oklab(Accent.usable(c).first))
            assertTrue("L ${lch[0]}", lch[0] in 0.735..0.865)
            assertTrue("C ${lch[1]}", lch[1] <= 0.161)
        }
    }

    @Test
    fun transitionKeepsItsColourBetweenComplementaryHues() {
        val a = Accent.usable(rgb(60, 120, 255)).first
        val b = Accent.usable(rgb(255, 160, 40)).first
        for (i in 0..10) {
            val m = Accent.mix(a, b, i / 10f)
            assertTrue("step $i: ${Integer.toHexString(m)} chroma ${chroma(m)}", chroma(m) > 0.06)
        }
        assertEquals(a, Accent.mix(a, b, 0f))
        assertEquals(b, Accent.mix(a, b, 1f))
    }

    @Test
    fun alphaInTheInputIsIgnored() {
        val opaque = image { x, _ -> if (x < 32) rgb(220, 40, 50) else rgb(10, 10, 10) }
        val withAlpha = IntArray(opaque.size) { opaque[it] or (0xFF shl 24) }
        assertEquals(Accent.extract(opaque), Accent.extract(withAlpha))
    }

    @Test
    fun formatsAndParsesForTheDiskCache() {
        assertEquals("b4a6e8", Accent.format(Accent.FALLBACK))
        assertEquals(Accent.FALLBACK, Accent.parse("b4a6e8"))
        assertEquals("none", Accent.format(null))
        assertNull(Accent.parse("none"))
    }

    @Test
    fun blurSmoothsAnEdgeAndKeepsAFlatImage() {
        val w = 16
        val flat = IntArray(w * w) { 0xFF336699.toInt() }
        assertTrue(Blur.boxBlur(flat.copyOf(), w, w, 3).all { it == 0xFF336699.toInt() })
        val edge = IntArray(w * w) { i -> if (i % w < w / 2) 0xFF000000.toInt() else 0xFFFFFFFF.toInt() }
        val out = Blur.boxBlur(edge, w, w, 2)
        val mid = out[8 * w + w / 2] and 0xFF
        assertTrue("middle $mid", mid in 60..200)
        assertEquals(0, out[8 * w] and 0xFF)
    }
}
