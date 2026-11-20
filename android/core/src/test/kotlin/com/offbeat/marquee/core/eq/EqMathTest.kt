package com.offbeat.marquee.core.eq

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.exp
import kotlin.math.ln
import kotlin.math.log10
import kotlin.math.log2
import kotlin.math.max
import kotlin.math.pow
import kotlin.math.sin
import kotlin.math.sqrt

/** Ported from the desktop's `crates/player-core/src/eq.rs` tests, `eq/presets.rs` tests and `tests/eq.rs`. */
class EqMathTest {
    private val rates = intArrayOf(44_100, 48_000, 96_000, 192_000, 384_000)
    private val SQRT_HALF = 0.7071067811865476

    private fun sine(freq: Double, rate: Int, frames: Int, amp: Double, channels: Int = 2): DoubleArray {
        val out = DoubleArray(frames * channels)
        for (n in 0 until frames) {
            val s = amp * sin(2.0 * PI * freq * n / rate)
            for (c in 0 until channels) out[n * channels + c] = s
        }
        return out
    }

    private fun rms(v: DoubleArray, from: Int = 0): Double {
        var sum = 0.0
        for (i in from until v.size) sum += v[i] * v[i]
        return sqrt(sum / (v.size - from))
    }

    private fun peak(v: DoubleArray, from: Int = 0, to: Int = v.size): Double {
        var p = 0.0
        for (i in from until to) p = max(p, abs(v[i]))
        return p
    }

    private fun settled(p: EqParams, rate: Int, channels: Int = 2) = Equalizer(rate, channels).also { it.setParams(p); it.settle() }

    @Test
    fun theCookbookCurvesHitTheirGains() {
        for (rate in rates) {
            val p = Coefs.of(Kind.Peak, 1000.0, 6.0, 1.0, rate)
            assertTrue("$rate", abs(p.gainDbAt(1000.0, rate) - 6.0) < 1e-9)
            val ls = Coefs.of(Kind.LowShelf, 100.0, -8.0, 0.707, rate)
            assertTrue("a shelf is half its gain at its midpoint", abs(ls.gainDbAt(100.0, rate) + 4.0) < 1e-6)
            assertTrue(abs(ls.gainDbAt(10.0, rate) + 8.0) < 0.2)
            val hs = Coefs.of(Kind.HighShelf, 8000.0, 4.0, 0.707, rate)
            assertTrue(abs(hs.gainDbAt(8000.0, rate) - 2.0) < 1e-6)
            assertTrue(abs(hs.gainDbAt(100.0, rate)) < 0.05)
            val fc = 1000.0
            assertTrue(abs(Coefs.of(Kind.LowPass, fc, 0.0, SQRT_HALF, rate).gainDbAt(fc, rate) + 10.0 * log10(2.0)) < 1e-9)
            assertTrue(abs(Coefs.of(Kind.HighPass, fc, 0.0, SQRT_HALF, rate).gainDbAt(fc, rate) + 10.0 * log10(2.0)) < 1e-9)
            assertTrue(abs(Coefs.of(Kind.BandPass, fc, 0.0, 2.0, rate).gainDbAt(fc, rate)) < 1e-9)
            assertTrue(Coefs.of(Kind.Notch, fc, 0.0, 2.0, rate).gainDbAt(fc, rate) < -100.0)
            assertTrue(Band.of(Kind.Notch, fc, 0.0, 2.0).powerAt(rate, fc) < 1e-20)
            for (f in listOf(20.0, 300.0, 1000.0, 7000.0)) assertTrue(abs(Coefs.of(Kind.AllPass, fc, 0.0, 0.7, rate).gainDbAt(f, rate)) < 1e-9)
        }
    }

    /** Every kind and width: the coefficients' response and the analog prototype at the warped frequency agree. */
    @Test
    fun theCoefficientsAreThePrototypesThroughTheBilinearTransform() {
        for (rate in rates) for (kind in Kind.entries) {
            for ((width, value) in listOf(Width.Q to 0.6, Width.Q to 3.0, Width.Octaves to 1.5, Width.Slope to 0.9)) {
                if (!width.fits(kind)) continue
                for ((freq, gain) in listOf(20.0 to -9.0, 440.0 to 6.0, 6000.0 to 3.0, 18_000.0 to -4.0)) {
                    val band = Band.of(kind, freq, gain, value, width)
                    val c = Coefs.of(band, rate)
                    for (f in listOf(50.0, 200.0, 1000.0, 4000.0, 12_000.0)) {
                        val fromCoefs = c.gainDbAt(f, rate)
                        if (fromCoefs < -80.0) continue
                        val analytic = 10.0 * log10(band.powerAt(rate, f))
                        assertTrue("$rate $kind $width $freq Hz at $f Hz: $fromCoefs vs $analytic", abs(fromCoefs - analytic) < 1e-6)
                    }
                }
            }
        }
    }

    @Test
    fun widthsAreEqualizerApos() {
        val b = Band.of(Kind.Peak, 1000.0, 6.0, 1.0, Width.Octaves)
        val q = b.qEquivalent(48_000)
        assertTrue("$q", abs(q - sqrt(2.0)) < 0.01 && q != sqrt(2.0))
        assertTrue(abs(Band.of(Kind.LowShelf, 200.0, 6.0, 1.0, Width.Slope).qEquivalent(48_000) - SQRT_HALF) < 1e-9)
        val steep = Band.of(Kind.HighShelf, 2000.0, 24.0, 2.0, Width.Slope)
        assertTrue(abs(steep.qEquivalent(48_000) - Q_MAX) < 1e-6)
        val e = settled(EqParams(true, listOf(steep), Preamp.Manual(-30.0)), 48_000)
        val s = sine(3000.0, 48_000, 48_000, 0.1)
        e.process(s)
        assertTrue(s.all { it.isFinite() && abs(it) < 4.0 })
        val x = Band.of(Kind.Peak, 200.0, 6.0, 0.9, Width.Slope)
        assertEquals(Width.Q, x.width)
        assertTrue(x.q > 0.0)
        assertEquals(0.0, Band.of(Kind.LowPass, 5000.0, 6.0, 0.7).gainDb, 0.0)
    }

    @Test
    fun zeroDbBandsPassSamplesThroughBitForBit() {
        for (kind in listOf(Kind.Peak, Kind.LowShelf, Kind.HighShelf)) {
            val c = Coefs.of(kind, 333.0, 0.0, 0.9, 44_100)
            assertEquals(1.0, c.b0, 0.0)
            assertTrue("$kind", c.b1 == c.a1 && c.b2 == c.a2)
        }
        for (kind in listOf(Kind.LowPass, Kind.HighPass, Kind.BandPass, Kind.Notch, Kind.AllPass)) {
            val c = Coefs.of(kind, 333.0, 0.0, 0.9, 44_100).mixed(0.0)
            assertEquals(1.0, c.b0, 0.0)
            assertTrue("$kind at mix 0", c.b1 == c.a1 && c.b2 == c.a2)
        }
        val eq = settled(EqParams(true, Graphic.bands(Graphic.FLAT)), 44_100)
        val input = sine(997.0, 44_100, 10_000, 0.7)
        val out = input.copyOf()
        eq.process(out)
        assertTrue("on and flat: untouched", out.contentEquals(input))
        // And through the 16-bit path: the same shorts, no dither added.
        val pcm = ShortArray(input.size) { (input[it] * 32767).toInt().toShort() }
        val pcmOut = ShortArray(pcm.size)
        eq.processPcm16(pcm, 0, pcmOut, 0, pcm.size / 2)
        assertTrue(pcm.contentEquals(pcmOut))
    }

    @Test
    fun autoPreampIsTheLargestBoost() {
        assertTrue(abs(maxBoostDb(listOf(Band.of(Kind.Peak, 1000.0, 6.0, 1.0)), 48_000) - 6.0) < 1e-3)
        val two = listOf(Band.of(Kind.Peak, 1000.0, 6.0, 1.0), Band.of(Kind.Peak, 1300.0, 6.0, 1.0))
        val m = maxBoostDb(two, 48_000)
        assertTrue("$m", m > 9.0)
        val brute = (0 until 2000).maxOf { responseDb(two, 48_000, 800.0 + it * 0.5) }
        assertTrue(abs(m - brute) < 0.01)
        assertEquals(0.0, maxBoostDb(listOf(Band.of(Kind.LowShelf, 100.0, -6.0, 0.7)), 48_000), 0.0)
        val lp = listOf(Band.of(Kind.LowPass, 2000.0, 0.0, 4.0))
        val mlp = maxBoostDb(lp, 48_000)
        assertTrue("$mlp", abs(mlp - (0 until 4000).maxOf { responseDb(lp, 48_000, 1000.0 + it * 0.5) }) < 0.01)
        assertTrue("$mlp", mlp > 11.9)
        val quiet = listOf(Band.of(Kind.LowPass, 2000.0, 0.0, 0.7), Band.of(Kind.Notch, 50.0, 0.0, 10.0), Band.of(Kind.BandPass, 900.0, 0.0, 3.0), Band.of(Kind.AllPass, 300.0, 0.0, 0.7))
        assertEquals(0.0, maxBoostDb(quiet, 48_000), 0.0)
        // The automatic preamp is minus it; a manual one stays as given.
        assertTrue(abs(EqParams(true, two).preampDb(48_000) + m) < 1e-12)
        assertEquals(-4.3, EqParams(true, two, Preamp.Manual(-4.3)).preampDb(48_000), 1e-12)
    }

    @Test
    fun aSineComesOutAtTheCurvesGain() {
        for (rate in intArrayOf(44_100, 48_000, 96_000, 192_000)) {
            val bands = listOf(Band.of(Kind.LowShelf, 105.0, 6.0, 0.7), Band.of(Kind.Peak, 3000.0, -4.0, 2.0), Band.of(Kind.HighShelf, 10_000.0, 3.0, 0.7))
            val params = EqParams(true, bands, Preamp.Manual(0.0))
            for (f in listOf(40.0, 105.0, 1000.0, 3000.0, 12_000.0)) {
                val eq = settled(params, rate)
                val input = sine(f, rate, rate, 0.25)
                val out = input.copyOf()
                eq.process(out)
                val tail = rate / 2 * 2
                val got = 20.0 * log10(rms(out, tail) / rms(input, tail))
                val want = responseDb(bands, rate, f)
                assertTrue("$rate Hz rate, $f Hz: $got vs $want", abs(got - want) < 0.01)
            }
        }
    }

    /** The graphic EQ measured: sines at each band centre come out at the drawn curve's gain (and the preamp's). */
    @Test
    fun graphicBandsMeasureAtTheirCentres() {
        val rate = 44_100
        val gains = listOf(6.0, -6.0, 3.0, -3.0, 12.0, -12.0, 4.5, -1.5, 9.0, -9.0)
        val bands = Graphic.bands(gains)
        val params = EqParams(true, bands)
        val pre = params.preampDb(rate)
        assertEquals(-maxBoostDb(bands, rate), pre, 1e-12)
        assertTrue("$pre", pre < -5.0 && pre > -14.0)
        for ((i, f) in Graphic.FREQS.withIndex()) {
            val eq = settled(params, rate)
            val input = sine(f, rate, rate, 0.2)
            val out = input.copyOf()
            eq.process(out)
            val tail = rate / 2 * 2
            val got = 20.0 * log10(rms(out, tail) / rms(input, tail))
            val want = responseDb(bands, rate, f) + pre
            assertTrue("band $i ($f Hz): $got vs $want", abs(got - want) < 0.02)
            // One slider alone is its own gain at its centre (with neighbours the overlap counts: the curve is the real one).
            val alone = Graphic.bands(List(10) { if (it == i) gains[i] else 0.0 })
            assertTrue("band $i alone", abs(responseDb(alone, rate, f) - gains[i]) < 0.01)
        }
        // The 16-bit path measures the same, to within the dither's noise.
        val eq = settled(params, rate)
        val f = 500.0
        val input = sine(f, rate, rate, 0.2)
        val pcm = ShortArray(input.size) { Math.round(input[it] * 32768.0).toInt().toShort() }
        val out = ShortArray(pcm.size)
        eq.processPcm16(pcm, 0, out, 0, rate)
        val o = DoubleArray(out.size) { out[it] / 32768.0 }
        val got = 20.0 * log10(rms(o, rate) / rms(input, rate))
        assertTrue("$got", abs(got - (responseDb(bands, rate, f) + pre)) < 0.02)
    }

    /** A +12 dB boost with the automatic preamp: full-scale sines never exceed full scale, and the 16-bit path never clips. */
    @Test
    fun aTwelveDbBoostWithHeadroomNeverClips() {
        val rate = 44_100
        for (gains in listOf(List(10) { if (it == 5) 12.0 else 0.0 }, List(10) { 12.0 }, listOf(12.0, 12.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 12.0, 12.0))) {
            val params = EqParams(true, Graphic.bands(gains))
            for (f in listOf(31.5, 100.0, 700.0, 1000.0, 1400.0, 4000.0, 12_000.0, 16_000.0)) {
                val eq = settled(params, rate)
                val input = sine(f, rate, rate, 0.999)
                val out = input.copyOf()
                eq.process(out)
                val p = peak(out, rate / 4 * 2)
                assertTrue("$gains at $f Hz: steady peak $p", p <= 1.0)
                // Music fades in; an abrupt start's transient is the one thing the clamp catches.
                val fade = rate / 50
                val pcm = ShortArray(input.size) { Math.round(input[it] * 32767.0 * minOf(1.0, (it / 2).toDouble() / fade)).toInt().toShort() }
                val dst = ShortArray(pcm.size)
                val eq16 = settled(params, rate)
                eq16.processPcm16(pcm, 0, dst, 0, rate)
                assertEquals("$gains at $f Hz: clipped samples", 0L, eq16.clipped)
            }
        }
    }

    /** Every preset at full scale (a log sweep 20 Hz to 20 kHz): the automatic preamp keeps it within the 2 % a sweep's transients add. */
    @Test
    fun noPresetClipsAFullScaleSweep() {
        val rate = 48_000
        val secs = 6.0
        val n = (secs * rate).toInt()
        val k = ln(20_000.0 / 20.0)
        val input = DoubleArray(n * 2)
        for (i in 0 until n) {
            val t = i.toDouble() / rate
            val s = 0.999 * sin(2.0 * PI * 20.0 * secs / k * (exp(t / secs * k) - 1.0))
            input[i * 2] = s; input[i * 2 + 1] = s
        }
        for (preset in Graphic.PRESETS + Graphic.Preset("All +12", List(10) { 12.0 })) {
            val out = input.copyOf()
            settled(EqParams(true, preset.bands), rate).process(out)
            val p = peak(out)
            assertTrue("${preset.name}: the sweep peaks at $p", p <= 1.02)
        }
    }

    @Test
    fun turnedOffItGlidesOutAndThenLeavesSamplesAlone() {
        for (bands in listOf(listOf(Band.of(Kind.Peak, 100.0, 9.0, 1.0)), listOf(Band.of(Kind.LowPass, 3000.0, 0.0, 0.7), Band.of(Kind.Notch, 200.0, 0.0, 4.0)))) {
            val eq = settled(EqParams(true, bands), 48_000)
            eq.process(sine(100.0, 48_000, 4800, 0.5))
            eq.setParams(EqParams(false, bands))
            for (i in 0 until 200) {
                eq.process(sine(100.0, 48_000, 480, 0.5))
                if (eq.isBypassed) break
            }
            assertTrue("bypassed within 2 s", eq.isBypassed)
            val input = sine(440.0, 48_000, 4800, 0.5)
            val out = input.copyOf()
            eq.process(out)
            assertTrue(out.contentEquals(input))
        }
    }

    private fun maxStep(v: DoubleArray): Double {
        var w = 0.0
        var i = 2
        while (i < v.size) { w = max(w, abs(v[i] - v[i - 2])); i += 2 }
        return w
    }

    /** A low-pass coming in, changing to a high-pass and going out again: no step larger than the steady signal's own. */
    @Test
    fun aFilterWithoutGainGlidesInAndOut() {
        val rate = 48_000
        val input = sine(1000.0, rate, rate * 2, 0.5)
        val steady = maxStep(input)
        val eq = Equalizer(rate, 2)
        val out = input.copyOf()
        val q = rate / 2
        eq.setParams(EqParams(true, listOf(Band.of(Kind.Peak, 1000.0, 0.0, 1.0)), Preamp.Manual(0.0)))
        eq.process(out, 0, q)
        eq.setParams(EqParams(true, listOf(Band.of(Kind.LowPass, 2000.0, 0.0, 2.0)), Preamp.Manual(0.0)))
        eq.process(out, q * 2, q)
        eq.setParams(EqParams(true, listOf(Band.of(Kind.HighPass, 3000.0, 0.0, 0.7)), Preamp.Manual(0.0)))
        eq.process(out, q * 4, q)
        eq.setParams(EqParams(false, listOf(Band.of(Kind.HighPass, 3000.0, 0.0, 0.7)), Preamp.Manual(0.0)))
        eq.process(out, q * 6, q)
        val worst = maxStep(out)
        val lifted = 10.0.pow(responseDb(listOf(Band.of(Kind.LowPass, 2000.0, 0.0, 2.0)), rate, 1000.0) / 20.0)
        assertTrue("$worst vs $steady x $lifted", worst <= steady * lifted * 1.02)
        val tail = out.size - 2000
        assertTrue("glided out and bypassed", (tail until out.size).all { out[it] == input[it] })
    }

    /** Moving a slider while playing, and switching on mid-track: no step in the signal beyond the louder steady level. */
    @Test
    fun sliderMovesAndSwitchingOnAreSmooth() {
        val rate = 44_100
        val f = 63.0
        val input = sine(f, rate, rate * 2, 0.5)
        val steady = maxStep(input)
        val eq = Equalizer(rate, 2)
        eq.glideInTo(EqParams(true, Graphic.bands(listOf(0.0, 6.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0))))
        val out = input.copyOf()
        // In blocks like the sink's, a slider jumping from +6 to -6 dB in the middle.
        var frame = 0
        var moved = false
        while (frame < rate * 2) {
            if (frame >= rate && !moved) {
                moved = true
                eq.setParams(EqParams(true, Graphic.bands(listOf(0.0, -6.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0))))
            }
            eq.process(out, frame * 2, minOf(1024, rate * 2 - frame))
            frame += 1024
        }
        val worst = maxStep(out)
        assertTrue("$worst vs steady $steady", worst <= steady * 10.0.pow(6.5 / 20.0))
        // And it got there: the last half second at the new setting's level.
        val want = responseDb(Graphic.bands(listOf(0.0, -6.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0)), rate, f) + eq.params.preampDb(rate)
        val got = 20.0 * log10(rms(out, (rate * 3 / 2) * 2) / rms(input, (rate * 3 / 2) * 2))
        assertTrue("$got vs $want", abs(got - want) < 0.02)
    }

    /** Another rate and channel count: worked out again, the same curve, the filters empty. */
    @Test
    fun formatChangesKeepTheCurve() {
        val bands = Graphic.bands(Graphic.preset("Bass boost")!!.gains)
        val eq = Equalizer(44_100, 2)
        eq.setParams(EqParams(true, bands, Preamp.Manual(0.0)))
        for ((rate, ch) in listOf(48_000 to 1, 44_100 to 2, 96_000 to 6, 22_050 to 2)) {
            eq.setFormat(rate, ch)
            val input = sine(63.0, rate, rate, 0.25, ch)
            val out = input.copyOf()
            eq.process(out)
            for (c in 0 until ch) {
                val got = 20.0 * log10(rmsChannel(out, ch, c, rate / 2) / rmsChannel(input, ch, c, rate / 2))
                assertTrue("$rate Hz x $ch, channel $c: $got", abs(got - responseDb(bands, rate, 63.0)) < 0.02)
            }
        }
    }

    private fun rmsChannel(v: DoubleArray, ch: Int, c: Int, fromFrame: Int): Double {
        var sum = 0.0
        var n = 0
        var i = fromFrame * ch + c
        while (i < v.size) { sum += v[i] * v[i]; n++; i += ch }
        return sqrt(sum / n)
    }

    // ---- Presets: still the fit of their targets (port of presets.rs's test) ----------------------

    /** The ten sliders closest to a curve: least squares every sixth of an octave, 31.5 Hz to 16 kHz (Gauss-Newton). */
    private fun fitGraphic(target: (Double) -> Double): DoubleArray {
        val lo = Graphic.FREQS[0]
        val hi = Graphic.FREQS[9]
        val nPts = Math.round(log2(hi / lo) * 6.0).toInt()
        val freqs = DoubleArray(nPts + 1) { lo * (hi / lo).pow(it.toDouble() / nPts) }
        val want = DoubleArray(freqs.size) { target(freqs[it]) }
        fun curve(g: DoubleArray) = responseCurve(Graphic.bands(g.toList()), 48_000, freqs)
        val g = DoubleArray(10) { target(Graphic.FREQS[it]).coerceIn(-12.0, 12.0) }
        repeat(12) {
            val base = curve(g)
            val r = DoubleArray(freqs.size) { want[it] - base[it] }
            val jac = Array(freqs.size) { DoubleArray(10) }
            for (j in 0 until 10) {
                val gj = g.copyOf(); gj[j] += 0.1
                val c = curve(gj)
                for (k in freqs.indices) jac[k][j] = (c[k] - base[k]) / 0.1
            }
            val a = Array(10) { DoubleArray(11) }
            for (i in 0 until 10) {
                for (j in 0 until 10) a[i][j] = freqs.indices.sumOf { jac[it][i] * jac[it][j] }
                a[i][i] += 1e-6
                a[i][10] = freqs.indices.sumOf { jac[it][i] * r[it] }
            }
            for (c in 0 until 10) {
                val p = (c until 10).maxBy { abs(a[it][c]) }
                val t = a[c]; a[c] = a[p]; a[p] = t
                for (row in 0 until 10) if (row != c) {
                    val f = a[row][c] / a[c][c]
                    for (x in c until 11) a[row][x] -= f * a[c][x]
                }
            }
            var step = 0.0
            for (i in 0 until 10) {
                val d = a[i][10] / a[i][i]
                g[i] = (g[i] + d).coerceIn(-12.0, 12.0)
                step = max(step, abs(d))
            }
            if (step < 1e-4) return DoubleArray(10) { Math.round(g[it] * 10.0) / 10.0 }
        }
        return DoubleArray(10) { Math.round(g[it] * 10.0) / 10.0 }
    }

    private fun target(name: String): (Double) -> Double {
        fun bandsCurve(vararg b: Band): (Double) -> Double { val l = b.toList(); return { x -> responseDb(l, 48_000, x) } }
        fun tilt(dbPerOctave: Double): (Double) -> Double = { f -> (-dbPerOctave * log2(f / 700.0)).coerceIn(-2.0, 2.0) }
        return when (name) {
            "Flat" -> { _ -> 0.0 }
            "Bass boost" -> bandsCurve(Band.of(Kind.LowShelf, 105.0, 6.0, 0.71))
            "Bass reducer" -> bandsCurve(Band.of(Kind.LowShelf, 105.0, -6.0, 0.71))
            "Treble boost" -> bandsCurve(Band.of(Kind.HighShelf, 6000.0, 4.0, 0.71))
            "Treble reducer" -> bandsCurve(Band.of(Kind.HighShelf, 6000.0, -4.0, 0.71))
            "Vocal" -> bandsCurve(Band.of(Kind.LowShelf, 150.0, -3.0, 0.71), Band.of(Kind.Peak, 2500.0, 3.0, 1.0))
            "Loudness" -> { f ->
                val pts = listOf(20.0 to 10.0, 31.5 to 9.0, 63.0 to 7.0, 125.0 to 5.0, 250.0 to 3.0, 500.0 to 1.0, 1000.0 to 0.0)
                if (f <= pts[0].first) pts[0].second else {
                    var out = 0.0
                    for (w in pts.windowed(2)) {
                        val (f0, g0) = w[0]; val (f1, g1) = w[1]
                        if (f <= f1) { var t = log2(f / f0) / log2(f1 / f0); t = t * t * (3.0 - 2.0 * t); out = g0 + (g1 - g0) * t; break }
                    }
                    out
                }
            }
            "Warm" -> tilt(0.45)
            "Bright" -> tilt(-0.45)
            "Spoken word" -> bandsCurve(Band.of(Kind.LowShelf, 120.0, -8.0, 0.71), Band.of(Kind.Peak, 2500.0, 3.0, 1.0), Band.of(Kind.HighShelf, 12_000.0, -2.0, 0.71))
            else -> error(name)
        }
    }

    @Test
    fun presetsFollowTheirTargets() {
        for (p in Graphic.PRESETS) {
            val t = target(p.name)
            val fitted = fitGraphic(t)
            for (i in 0 until 10) assertTrue("${p.name}: band $i is ${p.gains[i]}, the fit says ${fitted[i]}", abs(fitted[i] - p.gains[i]) < 0.051)
            var worst = 0.0
            for (i in 0..200) {
                val f = 31.5 * (16_000.0 / 31.5).pow(i / 200.0)
                worst = max(worst, abs(responseDb(p.bands, 48_000, f) - t(f)))
            }
            val allowed = if (p.name.startsWith("Treble")) 1.6 else 1.0
            assertTrue("${p.name}: $worst dB off its target", worst < allowed)
        }
    }

    @Test
    fun presetsAreRecognisedAndEditedOnesAreCustom() {
        assertEquals("Bass boost", Graphic.presetOf(Graphic.preset("bass boost")!!.gains)?.name)
        assertEquals("Flat", EqSetting().label)
        val edited = Graphic.preset("Vocal")!!.gains.toMutableList().also { it[3] = 2.0 }
        assertEquals(null, Graphic.presetOf(edited))
        assertEquals("Custom", EqSetting(gains = edited).label)
        assertEquals(10, Graphic.PRESETS.size)
        assertTrue(Graphic.PRESETS.map { it.name }.containsAll(listOf("Flat", "Bass boost", "Treble boost", "Vocal", "Loudness")))
    }

    // ---- What it costs ---------------------------------------------------------------------------

    /** 10 graphic bands, 44.1 kHz stereo, through the 16-bit path the sink uses: ns per frame (printed). */
    @Test
    fun whatItCosts() {
        val rate = 44_100
        val params = EqParams(true, Graphic.bands(listOf(3.0, -2.0, 1.0, 4.0, -3.0, 2.0, -1.0, 3.0, -2.0, 1.0)))
        val secs = 10
        val frames = rate * secs
        val src = ShortArray(frames * 2) { (sin(2.0 * PI * 440.0 * (it / 2) / rate) * 9000).toInt().toShort() }
        val dst = ShortArray(src.size)
        val block = 2048 // about what Media3 hands a processor at a time
        var best = Double.MAX_VALUE
        repeat(5) {
            val eq = settled(params, rate)
            val start = System.nanoTime()
            var f = 0
            while (f < frames) { val n = minOf(block, frames - f); eq.processPcm16(src, f * 2, dst, f * 2, n); f += n }
            best = minOf(best, (System.nanoTime() - start).toDouble() / frames)
        }
        val share = best * rate / 1e9 * 100.0
        println("EQ cost (JVM): 10 bands, 44.1 kHz stereo, 16-bit path: %.1f ns per frame (%.2f%% of one core)".format(best, share))
        // A new setting (the automatic preamp's search is the costly part).
        val t0 = System.nanoTime()
        val eq = Equalizer(rate, 2)
        repeat(100) { eq.setParams(params) }
        println("EQ cost (JVM): a new 10-band setting taken: %.3f ms".format((System.nanoTime() - t0) / 1e6 / 100))
        assertTrue("$best ns per frame", best < 5_000.0)
    }
}
