package com.offbeat.marquee.core.eq

import kotlin.math.abs

/**
 * The 10-band graphic layout and the built-in presets, as the desktop's `eq/presets.rs` has them.
 *
 * Peaking bands at the ISO octave centres 31.5 Hz to 16 kHz, each an octave wide (Q 1.41), ±12 dB.
 * Neighbouring bands overlap, so two sliders up together lift the octave between them by a little
 * more than either; the curve drawn is the real combined one.
 *
 * The presets are the desktop's, each fitted (least squares) to a target curve that says what it is
 * for; the test `presetsFollowTheirTargets` checks the fit still holds.
 */
object Graphic {
    val FREQS = doubleArrayOf(31.5, 63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16_000.0)
    val LABELS = listOf("31", "63", "125", "250", "500", "1k", "2k", "4k", "8k", "16k")
    /** Spoken by TalkBack. */
    val SPOKEN = listOf("31 hertz", "63 hertz", "125 hertz", "250 hertz", "500 hertz", "1 kilohertz", "2 kilohertz", "4 kilohertz", "8 kilohertz", "16 kilohertz")
    /** One octave wide: Q = sqrt(2) / (2 - 1). */
    const val Q = 1.41
    const val MAX_DB = 12.0
    const val COUNT = 10

    val FLAT: List<Double> = List(COUNT) { 0.0 }

    /** Ten sliders as bands. */
    fun bands(gains: List<Double>): List<Band> =
        FREQS.indices.map { i -> Band.of(Kind.Peak, FREQS[i], gains.getOrElse(i) { 0.0 }.coerceIn(-MAX_DB, MAX_DB), Q) }

    /** A built-in preset: its name and the ten sliders. */
    class Preset(val name: String, val gains: List<Double>) {
        val bands: List<Band> get() = bands(gains)
    }

    /** In the order the row lists them. */
    val PRESETS: List<Preset> = listOf(
        Preset("Flat", FLAT),
        Preset("Bass boost", listOf(5.4, 4.7, 1.1, -0.3, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0)),
        Preset("Bass reducer", listOf(-5.4, -4.7, -1.1, 0.3, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0)),
        Preset("Treble boost", listOf(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -0.1, 0.0, 3.6, 4.9)),
        Preset("Treble reducer", listOf(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.1, 0.0, -3.6, -4.9)),
        Preset("Vocal", listOf(-2.6, -2.5, -1.7, 0.1, 0.1, -0.1, 2.6, 1.4, -0.1, 0.0)),
        Preset("Loudness", listOf(8.4, 5.3, 3.7, 2.2, 0.5, -0.2, 0.0, 0.0, 0.0, 0.0)),
        Preset("Warm", listOf(1.8, 1.2, 0.8, 0.5, 0.2, -0.2, -0.5, -0.9, -1.6, -2.4)),
        Preset("Bright", listOf(-1.8, -1.2, -0.8, -0.5, -0.2, 0.2, 0.5, 0.9, 1.6, 2.4)),
        Preset("Spoken word", listOf(-7.1, -6.5, -2.6, 0.5, 0.1, 0.0, 2.6, 1.5, -0.3, -2.1)),
    )

    /** The preset these sliders are, or null once edited ("Custom"). */
    fun presetOf(gains: List<Double>): Preset? =
        PRESETS.firstOrNull { p -> p.gains.size == gains.size && p.gains.indices.all { abs(p.gains[it] - gains[it]) < 0.05 } }

    fun preset(name: String): Preset? = PRESETS.firstOrNull { it.name.equals(name, ignoreCase = true) }
}
