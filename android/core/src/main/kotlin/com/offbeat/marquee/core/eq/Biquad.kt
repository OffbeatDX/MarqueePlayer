package com.offbeat.marquee.core.eq

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.exp
import kotlin.math.ln
import kotlin.math.log10
import kotlin.math.log2
import kotlin.math.max
import kotlin.math.pow
import kotlin.math.sin
import kotlin.math.sinh
import kotlin.math.sqrt
import kotlin.math.tan

/*
 * The equalizer's bands, ported from the desktop's `crates/player-core/src/eq.rs`.
 *
 * Each band is a biquad from Robert Bristow-Johnson's "Audio EQ Cookbook" (what Equalizer APO and
 * AutoEq use): A = 10^(gain/40), w0 = 2 pi f0 / fs, alpha = sin(w0) / 2Q. A shelf's f0 is its
 * midpoint (half its gain in dB). A width is a Q, a bandwidth in octaves (the cookbook's digital
 * bandwidth, sinh form) or a shelf's slope S (APO's "x dB per octave" is S = x / 12). The
 * coefficients are worked out for the rate that plays; a band above 45% of it is held there.
 *
 * The curve (`responseDb`) comes from each band's analog prototype at the bilinear transform's
 * warped frequency, which is exactly the digital filter's response (the tests check the two
 * derivations agree) and loses no precision far below a high-pass's corner.
 */

const val MAX_BANDS = 32
const val FREQ_MIN = 10.0
const val FREQ_MAX = 24_000.0
const val GAIN_MAX = 24.0
const val Q_MIN = 0.1
const val Q_MAX = 50.0
const val OCT_MIN = 0.01
const val OCT_MAX = 8.0
const val SLOPE_MIN = 0.05
const val SLOPE_MAX = 2.0
const val PREAMP_MIN = -30.0
const val PREAMP_MAX = 12.0
private const val NYQUIST_SHARE = 0.45
private const val LN_2 = 0.6931471805599453

@Serializable
enum class Kind(val label: String) {
    @SerialName("peak") Peak("Peak"),
    @SerialName("lowShelf") LowShelf("Low shelf"),
    @SerialName("highShelf") HighShelf("High shelf"),
    @SerialName("lowPass") LowPass("Low-pass"),
    @SerialName("highPass") HighPass("High-pass"),
    /** Constant 0 dB peak gain (the cookbook's second band-pass), Equalizer APO's `BP`. */
    @SerialName("bandPass") BandPass("Band-pass"),
    @SerialName("notch") Notch("Notch"),
    @SerialName("allPass") AllPass("All-pass");

    /** Peaks and shelves have a gain; the others' level is fixed by their kind. */
    val hasGain: Boolean get() = this == Peak || this == LowShelf || this == HighShelf
    val isShelf: Boolean get() = this == LowShelf || this == HighShelf
}

/** How a band's width ([Band.q]) is given. */
@Serializable
enum class Width {
    @SerialName("q") Q,
    /** A bandwidth in octaves (not for shelves). */
    @SerialName("octaves") Octaves,
    /** A shelf's slope S (shelves only). */
    @SerialName("slope") Slope;

    fun fits(kind: Kind): Boolean = when (this) {
        Q -> true
        Octaves -> !kind.isShelf
        Slope -> kind.isShelf
    }

    val range: ClosedFloatingPointRange<Double>
        get() = when (this) {
            Q -> Q_MIN..Q_MAX
            Octaves -> OCT_MIN..OCT_MAX
            Slope -> SLOPE_MIN..SLOPE_MAX
        }
}

/** One band. Build it with [Band.of], which holds every value within the ranges. */
@Serializable
data class Band(
    val kind: Kind,
    /** Hz: a peak's centre, a shelf's midpoint, a pass filter's corner, a notch's centre. */
    val freq: Double,
    /** dB; 0 for the kinds without a gain. */
    val gainDb: Double = 0.0,
    /** The width, as [width] says: a Q, octaves or a shelf's slope S. */
    val q: Double,
    val width: Width = Width.Q,
) {
    /** Within the ranges (non-finite values become the defaults); a width its kind cannot have becomes the equivalent Q. */
    fun clamped(): Band {
        fun fin(v: Double, d: Double) = if (v.isFinite()) v else d
        val f = fin(freq, 1000.0).coerceIn(FREQ_MIN, FREQ_MAX)
        val g = if (kind.hasGain) fin(gainDb, 0.0).coerceIn(-GAIN_MAX, GAIN_MAX) else 0.0
        var b = Band(kind, f, g, q, width)
        if (!width.fits(kind)) {
            val asWas = Band(if (width == Width.Slope) Kind.LowShelf else Kind.Peak, f, g, q, width)
            b = b.copy(q = asWas.qEquivalent(48_000), width = Width.Q)
        }
        val r = b.width.range
        return b.copy(q = fin(b.q, 1.0).coerceIn(r.start, r.endInclusive))
    }

    /** The Q this band has at [rate]: sin(w0) / 2 alpha. */
    fun qEquivalent(rate: Int): Double = Proto.of(this, rate).q

    /** |H|^2 at [atFreq] Hz from the analog prototype at the warped frequency. */
    fun powerAt(rate: Int, atFreq: Double): Double = Proto.of(this, rate).powerAt(atFreq)

    companion object {
        fun of(kind: Kind, freq: Double, gainDb: Double, q: Double, width: Width = Width.Q) = Band(kind, freq, gainDb, q, width).clamped()
    }
}

/** A band's design at one rate: shared by the coefficients and the analytic response. */
internal class Proto(val kind: Kind, val fs: Double, val w0: Double, val sin: Double, val cos: Double, val alpha: Double, val a: Double) {
    val q: Double get() = sin / (2.0 * alpha)

    fun coefs(): Coefs {
        // 1 - cos(w0), without the cancellation of the subtraction at low frequencies.
        val s2 = sin(w0 / 2.0)
        val omc = 2.0 * s2 * s2
        val b0: Double; val b1: Double; val b2: Double; val a0: Double; val a1: Double; val a2: Double
        when (kind) {
            Kind.Peak -> { b0 = 1.0 + alpha * a; b1 = -2.0 * cos; b2 = 1.0 - alpha * a; a0 = 1.0 + alpha / a; a1 = -2.0 * cos; a2 = 1.0 - alpha / a }
            Kind.LowShelf -> {
                val s = 2.0 * sqrt(a) * alpha
                b0 = a * ((a + 1.0) - (a - 1.0) * cos + s)
                b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cos)
                b2 = a * ((a + 1.0) - (a - 1.0) * cos - s)
                a0 = (a + 1.0) + (a - 1.0) * cos + s
                a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cos)
                a2 = (a + 1.0) + (a - 1.0) * cos - s
            }
            Kind.HighShelf -> {
                val s = 2.0 * sqrt(a) * alpha
                b0 = a * ((a + 1.0) + (a - 1.0) * cos + s)
                b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cos)
                b2 = a * ((a + 1.0) + (a - 1.0) * cos - s)
                a0 = (a + 1.0) - (a - 1.0) * cos + s
                a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cos)
                a2 = (a + 1.0) - (a - 1.0) * cos - s
            }
            Kind.LowPass -> { b0 = omc / 2.0; b1 = omc; b2 = omc / 2.0; a0 = 1.0 + alpha; a1 = -2.0 * cos; a2 = 1.0 - alpha }
            Kind.HighPass -> { val opc = 2.0 - omc; b0 = opc / 2.0; b1 = -opc; b2 = opc / 2.0; a0 = 1.0 + alpha; a1 = -2.0 * cos; a2 = 1.0 - alpha }
            Kind.BandPass -> { b0 = alpha; b1 = 0.0; b2 = -alpha; a0 = 1.0 + alpha; a1 = -2.0 * cos; a2 = 1.0 - alpha }
            Kind.Notch -> { b0 = 1.0; b1 = -2.0 * cos; b2 = 1.0; a0 = 1.0 + alpha; a1 = -2.0 * cos; a2 = 1.0 - alpha }
            Kind.AllPass -> { b0 = 1.0 - alpha; b1 = -2.0 * cos; b2 = 1.0 + alpha; a0 = 1.0 + alpha; a1 = -2.0 * cos; a2 = 1.0 - alpha }
        }
        return Coefs(b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0)
    }

    /** |H|^2 at [freq] Hz: the analog prototype at Omega = tan(w / 2) / tan(w0 / 2). */
    fun powerAt(freq: Double): Double {
        val k = tan(w0 / 2.0)
        val om = tan(PI * freq / fs) / k
        val o2 = om * om
        val q = q
        val re = 1.0 - o2
        val im = om / q
        val den = re * re + im * im
        val p = when (kind) {
            Kind.LowPass -> 1.0 / den
            Kind.HighPass -> o2 * o2 / den
            Kind.BandPass -> im * im / den
            Kind.Notch -> re * re / den
            Kind.AllPass -> 1.0
            Kind.Peak -> { val n = im * a; val d = im / a; (re * re + n * n) / (re * re + d * d) }
            Kind.LowShelf -> { val x = om * sqrt(a) / q; a * a * ((a - o2).pow(2) + x * x) / ((1.0 - a * o2).pow(2) + x * x) }
            Kind.HighShelf -> { val x = om * sqrt(a) / q; a * a * ((1.0 - a * o2).pow(2) + x * x) / ((a - o2).pow(2) + x * x) }
        }
        return if (p.isFinite()) p else 0.0
    }

    companion object {
        fun of(b: Band, rate: Int) = of(b.kind, b.freq, b.gainDb, b.q, b.width, rate)

        fun of(kind: Kind, freq: Double, gainDb: Double, value: Double, width: Width, rate: Int): Proto {
            val fs = max(rate, 1).toDouble()
            val f0 = freq.coerceIn(FREQ_MIN, max(FREQ_MIN, fs * NYQUIST_SHARE))
            val g = if (kind.hasGain) gainDb else 0.0
            val a = 10.0.pow(g / 40.0)
            val w0 = 2.0 * PI * f0 / fs
            val s = sin(w0)
            val c = cos(w0)
            val alpha = when (width) {
                Width.Q -> s / (2.0 * max(value, Q_MIN))
                Width.Octaves -> s * sinh(LN_2 / 2.0 * max(value, OCT_MIN) * w0 / s)
                Width.Slope -> {
                    val sl = max(value, SLOPE_MIN)
                    // Past the steepest slope this gain allows the root would go negative: held where
                    // the shelf's Q is the largest the equalizer takes.
                    s / 2.0 * sqrt(max((a + 1.0 / a) * (1.0 / sl - 1.0) + 2.0, 1.0 / (Q_MAX * Q_MAX)))
                }
            }
            return Proto(kind, fs, w0, s, c, alpha, a)
        }
    }
}

/** One biquad, normalised (a0 = 1). */
data class Coefs(val b0: Double, val b1: Double, val b2: Double, val a1: Double, val a2: Double) {
    /** (1 - m) + m H: the same poles; the identity at m = 0 and this filter at m = 1. */
    fun mixed(m: Double): Coefs {
        if (m == 1.0) return this
        val d = 1.0 - m
        return Coefs(d + m * b0, d * a1 + m * b1, d * a2 + m * b2, a1, a2)
    }

    /** |H|^2 from phi = 4 sin^2(w/2) (the cookbook's form: nothing large cancels at low frequencies). */
    internal fun power(phi: Double): Double {
        val sb = b0 + b1 + b2
        val sa = 1.0 + a1 + a2
        val num = sb * sb - phi * (b1 * (b0 + b2) + 4.0 * b0 * b2) + phi * phi * b0 * b2
        val den = sa * sa - phi * (a1 * (1.0 + a2) + 4.0 * a2) + phi * phi * a2
        return num / den
    }

    /** The gain in dB at [freq] Hz, from the coefficients. */
    fun gainDbAt(freq: Double, rate: Int): Double {
        val w = 2.0 * PI * freq / max(rate, 1)
        return 10.0 * log10(max(power(phi(w)), 1e-30))
    }

    companion object {
        val IDENTITY = Coefs(1.0, 0.0, 0.0, 0.0, 0.0)
        fun of(kind: Kind, freq: Double, gainDb: Double, q: Double, rate: Int, width: Width = Width.Q) = Proto.of(kind, freq, gainDb, q, width, rate).coefs()
        fun of(b: Band, rate: Int) = Proto.of(b, rate).coefs()
    }
}

internal fun phi(w: Double): Double { val s = sin(w / 2.0); return 4.0 * s * s }

/** The combined curve of [bands] in dB at [freq] Hz (without the preamp). */
fun responseDb(bands: List<Band>, rate: Int, freq: Double): Double =
    bands.sumOf { 10.0 * log10(max(it.powerAt(rate, freq), 1e-30)) }

/** The combined curve at many frequencies at once (dB), for drawing. */
fun responseCurve(bands: List<Band>, rate: Int, freqs: DoubleArray): DoubleArray {
    val protos = bands.map { Proto.of(it, rate) }
    return DoubleArray(freqs.size) { i -> protos.sumOf { p -> 10.0 * log10(max(p.powerAt(freqs[i]), 1e-30)) } }
}

private const val SEARCH_PER_OCTAVE = 48.0

/** The band can never make any frequency louder. */
private fun neverBoosts(b: Band): Boolean = when (b.kind) {
    Kind.Peak, Kind.LowShelf, Kind.HighShelf -> b.gainDb <= 0.0
    Kind.BandPass, Kind.Notch, Kind.AllPass -> true
    Kind.LowPass, Kind.HighPass -> b.width == Width.Q && b.q <= 0.7071068
}

/**
 * The largest boost of the combined curve between 10 Hz and 20 kHz (or just under Nyquist), in dB;
 * 0 when it only cuts. Every 1/48 octave and at each band's own frequency, then refined around the
 * highest point (golden section), so within a few thousandths of a dB. The automatic preamp is
 * minus this: no frequency comes out louder than it went in.
 */
fun maxBoostDb(bands: List<Band>, rate: Int): Double {
    if (bands.all(::neverBoosts)) return 0.0
    val n = minOf(bands.size, MAX_BANDS)
    val coefs = Array(n) { Coefs.of(bands[it], rate) }
    val fs = max(rate, 1).toDouble()
    fun at(f: Double): Double {
        val p = phi(2.0 * PI * f / fs)
        var prod = 1.0
        for (c in coefs) prod *= c.power(p)
        return prod
    }
    val top = minOf(fs * 0.4999, 20_000.0)
    val lo = FREQ_MIN
    val steps = kotlin.math.ceil(log2(top / lo) * SEARCH_PER_OCTAVE).toInt()
    var bestF = 0.0
    var bestP = 1.0
    fun consider(f: Double) { val p = at(f); if (p > bestP) { bestF = f; bestP = p } }
    for (i in 0..steps) consider(lo * 2.0.pow(i / SEARCH_PER_OCTAVE))
    for (b in bands.take(n)) if (b.freq > lo && b.freq < top) consider(b.freq)
    if (bestF > 0.0) {
        val step = 2.0.pow(1.0 / SEARCH_PER_OCTAVE)
        var a = ln(max(bestF / step, lo))
        var b = ln(minOf(bestF * step, top))
        val g = (sqrt(5.0) - 1.0) / 2.0
        repeat(40) {
            val c = b - g * (b - a)
            val d = a + g * (b - a)
            if (at(exp(c)) > at(exp(d))) b = d else a = c
        }
        consider(exp((a + b) / 2.0))
    }
    return max(10.0 * log10(bestP), 0.0)
}
