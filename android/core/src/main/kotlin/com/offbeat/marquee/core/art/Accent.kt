package com.offbeat.marquee.core.art

import kotlin.math.abs
import kotlin.math.atan2
import kotlin.math.cbrt
import kotlin.math.cos
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.min
import kotlin.math.pow
import kotlin.math.roundToInt
import kotlin.math.sin

/**
 * The accent colour of a cover: a colour that belongs to the art, made safe to use as the UI
 * accent on a dark background. A port of the desktop's `crates/library/src/accent.rs`.
 *
 * [extract] picks the most prominent *vivid* hue (a plain average is a muddy grey-brown): every
 * pixel goes to OKLCH; near-black, near-white and near-grey pixels are ignored; the rest vote into
 * 24 hue sectors, weighted by chroma squared and by how close their lightness is to the middle. The
 * winning sector, smoothed with its neighbours, gives the hue; the colour is the vote-weighted mean
 * of that sector's pixels in Oklab. Art with too few vivid pixels has no accent.
 *
 * [usable] clamps lightness (0.74-0.86) and chroma (0.07-0.16) in OKLCH, then lowers chroma until
 * the colour is inside sRGB, so it reads on the dark background and under dark button text.
 *
 * Colours are 0xRRGGBB ints (any alpha byte in the input is ignored).
 */
object Accent {
    /** The accent when there is no art, the art is greyscale, or nothing plays. */
    const val FALLBACK: Int = 0xB4A6E8

    /** The accent for a cover's pixels (ARGB or RGB ints): extracted and clamped, or [FALLBACK]. */
    fun forPixels(pixels: IntArray): Int = extract(pixels)?.let { usable(it).first } ?: FALLBACK

    // ---- Colour spaces ---------------------------------------------------------------------

    private fun toLinear(c: Int): Double {
        val v = c / 255.0
        return if (v <= 0.04045) v / 12.92 else ((v + 0.055) / 1.055).pow(2.4)
    }

    private fun fromLinear(c: Double): Double {
        val v = c.coerceIn(0.0, 1.0)
        return if (v <= 0.0031308) v * 12.92 else 1.055 * v.pow(1.0 / 2.4) - 0.055
    }

    /** Oklab (L, a, b) from 0xRRGGBB. */
    fun oklab(rgb: Int): DoubleArray {
        val r = toLinear((rgb shr 16) and 0xFF)
        val g = toLinear((rgb shr 8) and 0xFF)
        val b = toLinear(rgb and 0xFF)
        val l = cbrt(0.41222147 * r + 0.53633255 * g + 0.051445995 * b)
        val m = cbrt(0.2119035 * r + 0.6806995 * g + 0.10739696 * b)
        val s = cbrt(0.08830246 * r + 0.28171885 * g + 0.6299787 * b)
        return doubleArrayOf(
            0.21045426 * l + 0.7936178 * m - 0.004072047 * s,
            1.9779985 * l - 2.4285922 * m + 0.4505937 * s,
            0.025904037 * l + 0.78277177 * m - 0.80867577 * s,
        )
    }

    private fun oklabToLinear(lab: DoubleArray): DoubleArray {
        val (l, a, b) = Triple(lab[0], lab[1], lab[2])
        val l_ = l + 0.39633778 * a + 0.21580376 * b
        val m_ = l - 0.105561346 * a - 0.06385417 * b
        val s_ = l - 0.08948418 * a - 1.2914855 * b
        val l3 = l_ * l_ * l_
        val m3 = m_ * m_ * m_
        val s3 = s_ * s_ * s_
        return doubleArrayOf(
            4.0767417 * l3 - 3.3077116 * m3 + 0.23096994 * s3,
            -1.268438 * l3 + 2.6097574 * m3 - 0.34131938 * s3,
            -0.0041960863 * l3 - 0.7034186 * m3 + 1.7076147 * s3,
        )
    }

    fun oklabToRgb(lab: DoubleArray): Int {
        val lin = oklabToLinear(lab)
        fun q(c: Double) = (fromLinear(c) * 255.0).roundToInt().coerceIn(0, 255)
        return (q(lin[0]) shl 16) or (q(lin[1]) shl 8) or q(lin[2])
    }

    /** OKLCH (L, C, h in radians) from Oklab. */
    fun toLch(lab: DoubleArray): DoubleArray = doubleArrayOf(lab[0], hypot(lab[1], lab[2]), atan2(lab[2], lab[1]))

    fun fromLch(lch: DoubleArray): DoubleArray = doubleArrayOf(lch[0], lch[1] * cos(lch[2]), lch[1] * sin(lch[2]))

    private fun inGamut(lab: DoubleArray): Boolean = oklabToLinear(lab).all { it in -0.0005..1.0005 }

    // ---- Extraction ------------------------------------------------------------------------

    private const val SECTORS = 24
    /** Pixels darker or lighter than this (Oklab L) do not vote: shadows and paper. */
    private const val MIN_L = 0.22
    private const val MAX_L = 0.975
    /** Pixels less colourful than this (Oklab chroma) do not vote: greys. */
    private const val MIN_C = 0.045
    /** Below this share of the image's possible vote, the art counts as colourless. */
    private const val MIN_SHARE = 0.001

    /** The most prominent vivid colour, unclamped (as it is in the art); null for colourless art. */
    fun extract(pixels: IntArray): Int? {
        val weight = DoubleArray(SECTORS)
        val sum = Array(SECTORS) { DoubleArray(3) }
        if (pixels.isEmpty()) return null
        for (px in pixels) {
            val lab = oklab(px)
            val lch = toLch(lab)
            val l = lch[0]
            val c = lch[1]
            if (l < MIN_L || l > MAX_L || c < MIN_C) continue
            // Chroma counts most; mid lightness a little more than the extremes.
            val w = c * c * (1.0 - abs(l - 0.6))
            val sector = (((lch[2] + Math.PI) / (2 * Math.PI)) * SECTORS).toInt() % SECTORS
            weight[sector] += w
            for (k in 0..2) sum[sector][k] += lab[k] * w
        }
        // Neighbouring sectors smooth the vote, so a hue on a sector boundary is not split.
        fun smoothed(i: Int) = weight[(i + SECTORS - 1) % SECTORS] * 0.5 + weight[i] + weight[(i + 1) % SECTORS] * 0.5
        var best = 0
        for (i in 1 until SECTORS) if (smoothed(i) > smoothed(best)) best = i
        // The strongest possible vote of one pixel is about 0.1 (chroma ~0.32 squared).
        if (smoothed(best) < MIN_SHARE * 0.1 * pixels.size || weight[best] <= 0.0) return null
        val w = weight[best]
        return oklabToRgb(doubleArrayOf(sum[best][0] / w, sum[best][1] / w, sum[best][2] / w))
    }

    // ---- Clamping --------------------------------------------------------------------------

    val L_RANGE = 0.74..0.86
    val C_RANGE = 0.07..0.16

    /**
     * A colour made usable as the accent, and the colour for text on accent-filled buttons (the
     * same hue, very dark).
     */
    fun usable(rgb: Int): Pair<Int, Int> {
        val lch = toLch(oklab(rgb))
        val l = lch[0].coerceIn(L_RANGE)
        var c = lch[1].coerceIn(C_RANGE)
        val h = lch[2]
        while (c > 0.0 && !inGamut(fromLch(doubleArrayOf(l, c, h)))) c -= 0.005
        val accent = oklabToRgb(fromLch(doubleArrayOf(l, max(c, 0.0), h)))
        val text = oklabToRgb(fromLch(doubleArrayOf(0.2, 0.035, h)))
        return accent to text
    }

    /** The text colour for an accent (dark, same hue). */
    fun onAccent(accent: Int): Int = oklabToRgb(fromLch(doubleArrayOf(0.2, 0.035, toLch(oklab(accent))[2])))

    // ---- Checks and transitions ------------------------------------------------------------

    /** WCAG relative luminance. */
    fun luminance(rgb: Int): Double =
        0.2126 * toLinear((rgb shr 16) and 0xFF) + 0.7152 * toLinear((rgb shr 8) and 0xFF) + 0.0722 * toLinear(rgb and 0xFF)

    fun contrast(a: Int, b: Int): Double {
        val la = luminance(a)
        val lb = luminance(b)
        return (max(la, lb) + 0.05) / (min(la, lb) + 0.05)
    }

    /**
     * One step of a perceptual transition: OKLCH, hue along the shorter way round, so a change
     * between complementary colours does not pass through grey.
     */
    fun mix(from: Int, to: Int, t: Float): Int {
        val k = t.coerceIn(0f, 1f).toDouble()
        val a = toLch(oklab(from))
        val b = toLch(oklab(to))
        // A near-grey end has no meaningful hue: take the other end's.
        var h0 = a[2]
        var h1 = b[2]
        if (a[1] < 0.02 && b[1] >= 0.02) h0 = h1 else if (b[1] < 0.02 && a[1] >= 0.02) h1 = h0
        var dh = h1 - h0
        if (dh > Math.PI) dh -= 2 * Math.PI else if (dh < -Math.PI) dh += 2 * Math.PI
        val l = a[0] + (b[0] - a[0]) * k
        var c = a[1] + (b[1] - a[1]) * k
        val h = h0 + dh * k
        var lab = fromLch(doubleArrayOf(l, c, h))
        // Keep intermediate steps in gamut the same way as the ends.
        while (c > 0.0 && !inGamut(lab)) {
            c -= 0.005
            lab = fromLch(doubleArrayOf(l, c, h))
        }
        return oklabToRgb(lab)
    }

    /** Parses what [format] writes: "rrggbb", or "none" for colourless art. */
    fun parse(text: String): Int? = text.trim().takeIf { it.length == 6 }?.toIntOrNull(16)

    fun format(rgb: Int?): String = rgb?.let { "%06x".format(it and 0xFFFFFF) } ?: "none"
}
