package com.offbeat.marquee.core.eq

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import kotlin.math.abs
import kotlin.math.pow

/** Ported from the desktop's `eq/apo.rs` tests and `tests/eq.rs` (`autoeq_s_files_play_as_written`). */
class ApoTest {
    /** AutoEq's results for the FiiO FT1 (MIT licence, `resources/autoeq/LICENSE-AutoEq.txt`). */
    private fun fixture(name: String): String =
        javaClass.getResourceAsStream("/autoeq/FiiO FT1 $name.txt")!!.use { it.readBytes().toString(Charsets.UTF_8) }

    private fun ok(text: String): Apo.Profile = when (val r = Apo.read(text)) {
        is Apo.Result.Ok -> r.profile
        is Apo.Result.Error -> { fail("$text: line ${r.line}: ${r.message}"); error("") }
    }

    private fun err(text: String): Apo.Result.Error = when (val r = Apo.read(text)) {
        is Apo.Result.Ok -> { fail("$text: read as ${r.profile}"); error("") }
        is Apo.Result.Error -> r
    }

    @Test
    fun autoEqsParametricFilePlaysAsWritten() {
        val p = ok(fixture("ParametricEQ"))
        assertEquals(10, p.bands.size)
        assertEquals(-4.3, p.preampDb!!, 1e-12)
        assertEquals(Band.of(Kind.LowShelf, 105.0, -2.7, 0.7), p.bands[0])
        assertEquals(Band.of(Kind.Peak, 3835.0, 4.6, 3.54), p.bands[3])
        assertEquals(Kind.HighShelf, p.bands[5].kind)
        assertTrue(p.notes.toString(), p.notes.isEmpty())
        assertEquals("FiiO FT1", Apo.nameFromFile("FiiO FT1 ParametricEQ.txt"))
        // Its own preamp covers its largest boost (AutoEq works it out the same way).
        val boost = maxBoostDb(p.bands, 48_000)
        assertTrue("$boost", boost in 0.0..4.3 + 0.05)
    }

    @Test
    fun autoEqsFixedBandFileIsTenPeaks() {
        val p = ok(fixture("FixedBandEQ"))
        assertEquals(10, p.bands.size)
        assertEquals(-2.8, p.preampDb!!, 1e-12)
        assertTrue(p.bands.all { it.kind == Kind.Peak && it.q == 1.41 })
        assertEquals(31.0, p.bands[0].freq, 0.0)
        assertEquals(-6.8, p.bands[9].gainDb, 1e-12)
        assertEquals("FiiO FT1", Apo.nameFromFile("FiiO FT1 FixedBandEQ"))
    }

    @Test
    fun aGraphicEqFileIsRefusedWithWhy() {
        val e = err(fixture("GraphicEQ"))
        assertTrue(e.message, e.message.contains("GraphicEQ"))
    }

    @Test
    fun aMalformedLineIsAnErrorWithItsNumber() {
        val lines = fixture("ParametricEQ").lines().toMutableList()
        lines[3] = "Filter 3: ON PK Fc Hz Gain 4.5 dB Q 1.99"
        val e = err(lines.joinToString("\n"))
        assertEquals(4, e.line)
        assertTrue(e.message, e.message.startsWith("Line 4") && e.message.contains("frequency"))
        val e2 = err("Preamp: -3 dB\nFilter 1: ON PK Fc 100 Hz Gain 3 dB\n")
        assertEquals(2, e2.line)
        assertTrue(e2.message.contains("no Q"))
        assertEquals(1, err("Preamp: loud\nFilter: ON PK Fc 100 Hz Gain 3 dB Q 1").line)
        assertEquals(3, err("Preamp: 0 dB\n\nFilter 2: maybe PK Fc 100 Hz Gain 3 dB Q 1").line)
        assertEquals(1, err("Filter: ON PK Fc 100 Hz Q 1").line)
    }

    /** Every example of Equalizer APO's configuration reference, as it reads them. */
    @Test
    fun readsTheReferencesExamples() {
        val cases = listOf(
            "Filter  1: ON  PK       Fc     50 Hz   Gain  -3.0 dB  Q 10.00" to Band.of(Kind.Peak, 50.0, -3.0, 10.0),
            "Filter  2: ON  PEQ      Fc     100 Hz  Gain   1.0 dB  BW Oct 0.167" to Band.of(Kind.Peak, 100.0, 1.0, 0.167, Width.Octaves),
            "Filter: ON  Modal    Fc     100 Hz  Gain   3.0 dB  Q  5.41  T60 target   100 ms" to Band.of(Kind.Peak, 100.0, 3.0, 5.41),
            "Filter: ON  LP       Fc   8000 Hz" to Band.of(Kind.LowPass, 8000.0, 0.0, 0.7071067811865476),
            "Filter  2: ON  LPQ      Fc     10000 Hz  Q  0.400" to Band.of(Kind.LowPass, 10_000.0, 0.0, 0.4),
            "Filter  1: ON  HP       Fc     30 Hz" to Band.of(Kind.HighPass, 30.0, 0.0, 0.7071067811865476),
            "Filter: ON  HPQ      Fc    20.0 Hz  Q  0.500" to Band.of(Kind.HighPass, 20.0, 0.0, 0.5),
            "Filter: ON BP Fc 1000 Hz Q 0.100" to Band.of(Kind.BandPass, 1000.0, 0.0, 0.1),
            "Filter  1: ON  LS       Fc     300 Hz  Gain   5.0 dB" to Band.of(Kind.LowShelf, 300.0, 5.0, 0.9, Width.Slope),
            "Filter: ON  LSC 10.8 dB Fc     300 Hz  Gain   5.0 dB" to Band.of(Kind.LowShelf, 300.0, 5.0, 0.9, Width.Slope),
            "Filter: ON  LSC Fc     300 Hz  Gain   5.0 dB Q 0.6473" to Band.of(Kind.LowShelf, 300.0, 5.0, 0.6473),
            "Filter: ON  HS       Fc   1000 Hz  Gain  -3.0 dB" to Band.of(Kind.HighShelf, 1000.0, -3.0, 0.9, Width.Slope),
            "Filter: ON  HSC 6 dB Fc     100 Hz  Gain   -6.0 dB" to Band.of(Kind.HighShelf, 100.0, -6.0, 0.5, Width.Slope),
            "Filter: ON HSC Fc 100 Hz Gain -6.0 dB Q 0.4272" to Band.of(Kind.HighShelf, 100.0, -6.0, 0.4272),
            "Filter: ON  NO       Fc     800 Hz" to Band.of(Kind.Notch, 800.0, 0.0, 30.0),
            "Filter: ON  AP       Fc     900 Hz  Q  0.707" to Band.of(Kind.AllPass, 900.0, 0.0, 0.707),
        )
        for ((line, want) in cases) {
            val got = ok(line).bands
            assertEquals(line, 1, got.size)
            val b = got[0]
            assertEquals(line, want.kind, b.kind)
            assertEquals(line, want.width, b.width)
            assertEquals(line, want.freq, b.freq, 1e-9)
            assertEquals(line, want.gainDb, b.gainDb, 1e-9)
            assertEquals(line, want.q, b.q, 1e-9)
        }
        // The corner-frequency shelves: moved to their midpoint by 10^(|g| / 80 / S).
        val p = ok("Filter: ON  LS 6dB   Fc    50.0 Hz  Gain   7.2 dB").bands[0]
        assertEquals(50.0 * 10.0.pow(7.2 / 80.0 / 0.5), p.freq, 0.01)
        assertEquals(Width.Slope, p.width)
        assertEquals(0.5, p.q, 1e-12)
        assertEquals(2000.0 * 10.0.pow(5.0 / 80.0), ok("Filter: ON  LS 12dB  Fc   2000 Hz  Gain  -5.0 dB").bands[0].freq, 0.1)
        assertEquals(500.0 / 10.0.pow(5.0 / 80.0), ok("Filter: ON  HS 12dB  Fc     500 Hz  Gain   5.0 dB").bands[0].freq, 0.01)
        assertEquals(12_000.0 / 10.0.pow(10.0 / 40.0), ok("Filter: ON  HS 6dB   Fc  12000 Hz  Gain  10.0 dB").bands[0].freq, 0.1)
        val lsq = ok("Filter: ON LS Fc 300 Hz Gain 6 dB Q 0.7071").bands[0]
        assertEquals(300.0 * 10.0.pow(6.0 / 80.0 / 1.0), lsq.freq, 0.05)
        assertEquals(Width.Q, lsq.width)
    }

    @Test
    fun readsTheVariantsEqualizerApoAllows() {
        val text = "# headphones\r\npreamp: -3 db\nPreamp: -1,5 dB\nFilter: on pk fc 1000 hz gain 3 db bw oct 1\nFilter  2: OFF PK Fc 50 Hz Gain -10 dB Q 2.5\nFilter3: ON PK Fc 1,000 Hz Gain -1,5 dB Q 2\nChannel: all\nFilter 4: ON None\n"
        val p = ok(text)
        assertEquals(-4.5, p.preampDb!!, 1e-12)
        assertEquals(2, p.bands.size)
        assertEquals(Width.Octaves, p.bands[0].width)
        assertEquals(1.0, p.bands[0].q, 0.0)
        assertEquals("1,000 is 1.000, read as a thousand", 1000.0, p.bands[1].freq, 0.0)
        assertEquals(-1.5, p.bands[1].gainDb, 1e-12)
        assertTrue(p.notes.toString(), p.notes.any { it.contains("OFF") })
        assertTrue(p.notes.toString(), p.notes.any { it.contains("thousands") })
    }

    @Test
    fun perChannelAndPerDevicePartsAreLeftOutAndSaid() {
        val text = "Preamp: -3 dB\nFilter 1: ON PK Fc 100 Hz Gain 3 dB Q 1\nChannel: L\nPreamp: -5 dB\nFilter 2: ON PK Fc 200 Hz Gain 2 dB Q 1\nChannel: C LFE\nFilter 3: ON HP Fc 30 Hz\nChannel: L R\nFilter 4: ON PK Fc 400 Hz Gain 1 dB Q 1\nDevice: Speakers\nFilter 5: ON PK Fc 800 Hz Gain 1 dB Q 1\nDevice: all\nFilter 6: ON PK Fc 1600 Hz Gain 1 dB Q 1\n"
        val p = ok(text)
        assertEquals(listOf(100.0, 400.0, 1600.0), p.bands.map { it.freq })
        assertEquals(-3.0, p.preampDb!!, 1e-12)
        assertEquals(p.notes.toString(), 4, p.notes.count { it.contains("left out") })
        assertTrue(p.notes[0].contains("left channel"))
        assertTrue(p.notes.any { it.contains("Speakers") })
    }

    @Test
    fun refusesWhatItCannotPlayAndSkipsWhatItDoesNotHave() {
        for ((text, says) in listOf(
            "Include: other.txt" to "includes another file",
            "Convolution: church.wav" to "convolution",
            "If: sampleRate == 44100\nFilter: ON PK Fc 100 Hz Gain 3 dB Q 1\nEndIf:" to "expression",
            "Filter: ON PK Fc 1000 Hz Gain `20*log10(0.5)` dB Q 10.0" to "inline expression",
            "Preamp: -3 dB" to "No filters",
            "Filter 1: OFF PK Fc 100 Hz Gain 3 dB Q 1\nFilter 2: OFF PK Fc 200 Hz Gain 3 dB Q 1" to "OFF",
            "hello\nworld" to "no equalizer settings",
            "Time: 10:30\nhttp://example.com/a:b" to "no equalizer settings",
        )) {
            val e = err(text)
            assertTrue("$text: ${e.message}", e.message.contains(says))
        }
        // Skipped with a note, the rest read.
        val p = ok("Filter 1: ON PK Fc 100 Hz Gain 3 dB Q 1\nFilter 2: ON XYZ Fc 100 Hz\nFilter: ON IIR Order 2 Coefficients 0.03 0.07 0.03 1.27 -1.84 0.72\nCopy: L=R\nDelay: 5 ms\nGraphicEQ: 20 1; 100 2")
        assertEquals(1, p.bands.size)
        assertEquals(p.notes.toString(), 5, p.notes.size)
        assertTrue(p.notes[0].contains("XYZ") && p.notes[0].startsWith("Line 2"))
        assertTrue(p.notes[1].contains("IIR"))
        assertNull(p.preampDb)
    }

    @Test
    fun hostileTextIsBoundedAndRefusedPlainly() {
        val many = (0 until 40).joinToString("\n") { "Filter $it: ON PK Fc ${100 + it} Hz Gain 3 dB Q 1" }
        assertTrue(err(many).message.contains("up to 32"))
        val huge = (0 until 50_000).joinToString("\n") { "Filter $it: ON PK Fc ${100 + it} Hz Gain 1e308 dB Q 1" }
        assertTrue(err(huge).message.contains("too long"))
        val held = ok("Filter: ON PK Fc 100000 Hz Gain 99 dB Q 1")
        assertTrue(held.bands[0].gainDb == 24.0 && held.bands[0].freq == 24_000.0)
        assertTrue(held.notes.any { it.contains("held within") })
        for (junk in listOf("Filter: ON PK Fc 1e999 Hz Gain nan dB Q inf", "Filter: ON PK Fc -- Hz Gain .. dB Q ..", "Filter: ON", "Filter:", "Preamp: 1e400 dB", "Filter: ON PK Fc 100 Hz Gain 3 dB Q -1")) {
            Apo.read(junk) // never throws
        }
    }

    @Test(timeout = 5_000)
    fun aLongRunOfDigitsIsReadQuickly() {
        val p = ok("Preamp: " + "1".repeat(250_000) + " dB\nFilter: ON PK Fc 100 Hz Gain 3 dB Q 1")
        assertEquals(PREAMP_MAX, p.preampDb!!, 0.0)
        assertEquals(1.5, Apo.floatPrefix("1.5" + "0".repeat(100))!!, 0.0)
    }

    @Test
    fun theProfilesPreampIsHeldInRangeAndSaid() {
        val low = ok("Preamp: -20 dB\nPreamp: -20 dB\nFilter: ON PK Fc 100 Hz Gain 3 dB Q 1")
        assertEquals(PREAMP_MIN, low.preampDb!!, 0.0)
        assertTrue(low.notes.any { it.contains("preamp") && it.contains("held within") })
        val high = ok("Preamp: 20 dB\nFilter: ON PK Fc 100 Hz Gain 3 dB Q 1")
        assertEquals(PREAMP_MAX, high.preampDb!!, 0.0)
        val fine = ok("Preamp: -6 dB\nFilter: ON PK Fc 100 Hz Gain 3 dB Q 1")
        assertTrue(fine.notes.none { it.contains("preamp") })
        // A stored setting is held in range too.
        val stored = EqSetting(mode = EqMode.Profile, profile = EqProfile("x", fine.bands, -40.0)).encode()
        assertEquals(PREAMP_MIN, EqSetting.decode(stored).profile!!.preampDb!!, 0.0)
    }

    @Test
    fun aSettingRoundTripsAndPlaysWhatItSays() {
        val profile = ok(fixture("ParametricEQ"))
        val s = EqSetting(enabled = true, mode = EqMode.Profile, gains = Graphic.preset("Loudness")!!.gains, profile = EqProfile("FiiO FT1", profile.bands, profile.preampDb, profile.notes))
        val back = EqSetting.decode(s.encode())
        assertEquals(s, back)
        assertEquals("FiiO FT1", back.label)
        assertEquals(Preamp.Manual(-4.3), back.params().preamp)
        assertEquals(10, back.params().bands.size)
        val graphic = back.copy(mode = EqMode.Graphic)
        assertEquals("Loudness", graphic.label)
        assertEquals(Preamp.Auto, graphic.params().preamp)
        assertEquals(EqSetting(), EqSetting.decode("{not json"))
        assertEquals(EqSetting(), EqSetting.decode(null))
        assertTrue(!EqSetting().enabled)
        // Out-of-range stored gains are held in range.
        assertEquals(12.0, EqSetting.decode("""{"gains":[99,0,0,0,0,0,0,0,0,0]}""").gains[0], 0.0)
        assertTrue(abs(EqSetting.decode("""{"gains":[1]}""").gains.size - 10.0) < 1e-9)
    }
}
