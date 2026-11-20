package com.offbeat.marquee.core.eq

import kotlin.math.abs
import kotlin.math.exp
import kotlin.math.ln
import kotlin.math.log2
import kotlin.math.pow
import kotlin.math.roundToInt

/** How loud the input is made before the bands. */
sealed interface Preamp {
    /** Down by the curve's largest boost ([maxBoostDb]), so nothing comes out louder than it went in; 0 dB for a curve that only cuts. */
    data object Auto : Preamp
    /** This many dB, whatever the curve (an imported profile's `Preamp:` line). */
    data class Manual(val db: Double) : Preamp
}

/** What the equalizer does: on or off, up to [MAX_BANDS] bands, and the preamp. */
class EqParams(val enabled: Boolean, bands: List<Band>, preamp: Preamp = Preamp.Auto) {
    val bands: List<Band> = bands.take(MAX_BANDS).map { it.clamped() }
    val preamp: Preamp = when (preamp) {
        is Preamp.Manual -> Preamp.Manual(if (preamp.db.isFinite()) preamp.db.coerceIn(PREAMP_MIN, PREAMP_MAX) else 0.0)
        Preamp.Auto -> Preamp.Auto
    }

    /** The preamp in dB at [rate] (the automatic one depends on the curve). */
    fun preampDb(rate: Int): Double = when (val p = preamp) {
        Preamp.Auto -> -maxBoostDb(bands, rate)
        is Preamp.Manual -> p.db
    }

    companion object {
        val OFF = EqParams(false, emptyList())
    }
}

/**
 * The equalizer as the audio thread runs it: a cascade of biquads in double precision, direct form I
 * with each delay's feedforward and feedback terms paired (`b1 x1 - a1 y1`), so a 0 dB band passes
 * its input bit for bit and a coefficient change acts on real history. Port of the desktop's
 * `Equalizer` (`crates/player-core/src/eq.rs`), for any channel count up to [MAX_CHANNELS].
 *
 * **Changes while playing.** A new setting is a target: gain, log frequency and log width glide to it,
 * recomputed every [SUB] frames with a one-pole of [GLIDE_SECS]; the preamp is interpolated per
 * sample. A filter without a gain glides in by its mix, `(1 - m) + m H`. Turned off, everything
 * glides to the identity and then [isBypassed] is true: the samples are left alone.
 *
 * Nothing allocates after construction (and [setFormat]).
 */
class Equalizer(rate: Int = 48_000, channels: Int = 2) {
    var rate: Int = rate
        private set
    var channels: Int = channels.coerceIn(1, MAX_CHANNELS)
        private set

    private var target: EqParams = EqParams.OFF
    private val slots = Array(MAX_BANDS) { Slot() }
    private var pre = 1.0
    private var preTargetDb = 0.0
    private var preDb = 0.0
    /** The cascade's last two input frames (after the preamp): [c * 2] = x1, [c * 2 + 1] = x2. */
    private var hist = DoubleArray(this.channels * 2)
    private var prevOut = DoubleArray(this.channels * 2)
    private var gliding = false
    /** Nothing has gone through since the filters were emptied: a new setting applies at once. */
    private var fresh = true
    private var work = DoubleArray(SUB * this.channels)
    private var dither = 0x2545F491

    /** Samples held at full scale by [processPcm16] since the last [reset] (the headroom's failures). */
    var clipped: Long = 0
        private set

    val params: EqParams get() = target

    /** A new setting: everything glides there from where it is (at once when nothing has played since a reset). */
    fun setParams(p: EqParams) {
        target = p
        preTargetDb = if (p.enabled) p.preampDb(rate) else 0.0
        gliding = true
        if (fresh) jump()
    }

    /** Empty filters at the identity, then a glide to [p] (an equalizer switched on mid-track fades in, no step). */
    fun glideInTo(p: EqParams) {
        target = EqParams.OFF
        preTargetDb = 0.0
        reset()
        fresh = false
        setParams(p)
    }

    /** Another rate or channel count: the bands are worked out again and the filters start empty. */
    fun setFormat(rate: Int, channels: Int) {
        val ch = channels.coerceIn(1, MAX_CHANNELS)
        if (rate == this.rate && ch == this.channels) return
        this.rate = rate
        if (ch != this.channels) {
            this.channels = ch
            hist = DoubleArray(ch * 2)
            prevOut = DoubleArray(ch * 2)
            work = DoubleArray(SUB * ch)
            for (s in slots) s.st = DoubleArray(ch * 4)
        }
        preTargetDb = if (target.enabled) target.preampDb(rate) else 0.0
        reset()
    }

    /** The stream restarts elsewhere (a seek, another track): empty filters, the setting applied as it is. */
    fun reset() {
        for (s in slots) s.st.fill(0.0)
        hist.fill(0.0)
        jump()
        fresh = true
        clipped = 0
    }

    /** Settled at the target now, no glide (tests, and a measurement's start). */
    fun settle() = jump()

    /** Off, and everything back at the identity: [process] leaves the samples alone. */
    val isBypassed: Boolean get() = !target.enabled && !gliding && pre == 1.0 && slots.all { !it.active }

    private inner class Slot {
        var active = false
        var kind = Kind.Peak
        var width = Width.Q
        /** The gain in dB for a peak or shelf, the mix (0 to 1) for the others; 0 is the identity. */
        var level = 0.0
        var lf = 10.0
        var lq = 0.0
        var b0 = 1.0; var b1 = 0.0; var b2 = 0.0; var a1 = 0.0; var a2 = 0.0
        /** Per channel: x1, x2, y1, y2. */
        var st = DoubleArray(channels * 4)

        fun setCoefs(c: Coefs) { b0 = c.b0; b1 = c.b1; b2 = c.b2; a1 = c.a1; a2 = c.a2 }

        fun transparent(): Boolean {
            if (level != 0.0) return false
            var i = 0
            while (i < st.size) { if (st[i] != st[i + 2] || st[i + 1] != st[i + 3]) return false; i += 4 }
            return true
        }
    }

    private fun slotCoefs(kind: Kind, width: Width, level: Double, lf: Double, lq: Double): Coefs =
        if (kind.hasGain) Coefs.of(kind, 2.0.pow(lf), level, exp(lq), rate, width)
        else Coefs.of(kind, 2.0.pow(lf), 0.0, exp(lq), rate, width).mixed(level)

    // The target of slot i, without allocating.
    private var tKind = Kind.Peak
    private var tWidth = Width.Q
    private var tLevel = 0.0
    private var tLf = 0.0
    private var tLq = 0.0
    private var tWanted = false

    private fun slotTarget(i: Int) {
        val t = target
        if (i < t.bands.size) {
            val b = t.bands[i]
            tKind = b.kind; tWidth = b.width
            tLevel = if (!t.enabled) 0.0 else if (b.kind.hasGain) b.gainDb else 1.0
            tLf = log2(b.freq); tLq = ln(b.q); tWanted = t.enabled
        } else {
            val s = slots[i]
            tKind = s.kind; tWidth = s.width; tLevel = 0.0; tLf = s.lf; tLq = s.lq; tWanted = false
        }
    }

    private fun jump() {
        for (i in 0 until MAX_BANDS) {
            slotTarget(i)
            val s = slots[i]
            s.kind = tKind; s.width = tWidth; s.level = tLevel; s.lf = tLf; s.lq = tLq; s.active = tWanted
            s.setCoefs(slotCoefs(tKind, tWidth, tLevel, tLf, tLq))
        }
        preDb = preTargetDb
        pre = 10.0.pow(preDb / 20.0)
        gliding = false
    }

    /** One glide step (every [SUB] frames while gliding). */
    private fun step() {
        val k = 1.0 - exp(-SUB.toDouble() / (GLIDE_SECS * maxOf(rate, 1)))
        var moving = false
        var havePrev = false
        val ch = channels
        for (i in 0 until MAX_BANDS) {
            slotTarget(i)
            val s = slots[i]
            val h = if (havePrev) prevOut else hist
            if (!s.active) {
                if (!tWanted) continue
                // A new band: in at the identity, its state the input it would have had.
                s.active = true; s.kind = tKind; s.width = tWidth; s.level = 0.0; s.lf = tLf; s.lq = tLq
                for (c in 0 until ch) {
                    s.st[c * 4] = h[c * 2]; s.st[c * 4 + 1] = h[c * 2 + 1]; s.st[c * 4 + 2] = h[c * 2]; s.st[c * 4 + 3] = h[c * 2 + 1]
                }
                s.setCoefs(slotCoefs(tKind, tWidth, 0.0, tLf, tLq))
                moving = true
            }
            // Changing kind (or how the width is given): to the identity first, where every kind is the same.
            val tlNow = if (s.kind != tKind || s.width != tWidth) 0.0 else tLevel
            val bLevel = s.level; val bLf = s.lf; val bLq = s.lq; val bKind = s.kind; val bWidth = s.width
            if (s.level != tlNow) {
                s.level += (tlNow - s.level) * k
                val near = if (s.kind.hasGain) 0.002 else 0.0005
                if (abs(tlNow - s.level) < near) s.level = tlNow
            }
            if (s.level == 0.0 && (s.kind != tKind || s.width != tWidth || s.lf != tLf || s.lq != tLq)) {
                s.kind = tKind; s.width = tWidth; s.lf = tLf; s.lq = tLq
            } else {
                if (s.lf != tLf) { s.lf += (tLf - s.lf) * k; if (abs(tLf - s.lf) < 0.0005) s.lf = tLf }
                if (s.lq != tLq) { s.lq += (tLq - s.lq) * k; if (abs(tLq - s.lq) < 0.0005) s.lq = tLq }
            }
            if (s.level != bLevel || s.lf != bLf || s.lq != bLq || s.kind != bKind || s.width != bWidth) {
                s.setCoefs(slotCoefs(s.kind, s.width, s.level, s.lf, s.lq))
            }
            if (s.level != tLevel || s.kind != tKind || s.width != tWidth || s.lf != tLf || s.lq != tLq) moving = true
            if (s.level == 0.0) {
                // At the identity it still rings out what it did; once that is far below audible, its
                // outputs are its inputs again (and it can go).
                var settled = true
                for (c in 0 until ch) {
                    if (abs(s.st[c * 4] - s.st[c * 4 + 2]) >= 1e-9 || abs(s.st[c * 4 + 1] - s.st[c * 4 + 3]) >= 1e-9) { settled = false; break }
                }
                if (settled) {
                    for (c in 0 until ch) { s.st[c * 4 + 2] = s.st[c * 4]; s.st[c * 4 + 3] = s.st[c * 4 + 1] }
                    if (!tWanted) s.active = false
                } else {
                    moving = true
                }
            }
            if (s.active) {
                for (c in 0 until ch) { prevOut[c * 2] = s.st[c * 4 + 2]; prevOut[c * 2 + 1] = s.st[c * 4 + 3] }
                havePrev = true
            }
        }
        if (preDb != preTargetDb) {
            preDb += (preTargetDb - preDb) * k
            if (abs(preTargetDb - preDb) < 0.002) preDb = preTargetDb
            moving = true
        }
        gliding = moving
    }

    /** Equalize interleaved samples (full scale ±1) in place: [frames] frames of [channels] from [offset]. */
    fun process(buf: DoubleArray, offset: Int = 0, frames: Int = (buf.size - offset) / channels) {
        if (isBypassed) return
        fresh = false
        var f = 0
        while (f < frames) {
            val n = minOf(SUB, frames - f)
            processBlock(buf, offset + f * channels, n)
            f += n
        }
    }

    /**
     * 16-bit in, 16-bit out (what Media3's sink hands its processors): converted to double, equalized,
     * then back with TPDF dither and held within full scale (counted in [clipped]). Blocks the
     * equalizer leaves untouched are copied as they are, bit for bit, with no dither.
     */
    fun processPcm16(src: ShortArray, srcOffset: Int, dst: ShortArray, dstOffset: Int, frames: Int) {
        val ch = channels
        if (isBypassed) {
            System.arraycopy(src, srcOffset, dst, dstOffset, frames * ch)
            return
        }
        fresh = false
        var f = 0
        val w = work
        while (f < frames) {
            val n = minOf(SUB, frames - f)
            val si = srcOffset + f * ch
            val di = dstOffset + f * ch
            val len = n * ch
            for (j in 0 until len) w[j] = src[si + j] * (1.0 / 32768.0)
            if (!processBlock(w, 0, n)) {
                System.arraycopy(src, si, dst, di, len)
            } else {
                var r = dither
                for (j in 0 until len) {
                    // Triangular dither of ±1 LSB: two uniform draws (xorshift32).
                    r = r xor (r shl 13); r = r xor (r ushr 17); r = r xor (r shl 5)
                    val u1 = (r ushr 8) * (1.0 / 16777216.0)
                    r = r xor (r shl 13); r = r xor (r ushr 17); r = r xor (r shl 5)
                    val u2 = (r ushr 8) * (1.0 / 16777216.0)
                    val v = (w[j] * 32768.0 + (u1 - u2)).roundToInt()
                    dst[di + j] = when {
                        v > 32767 -> { clipped++; 32767 }
                        v < -32768 -> { clipped++; -32768 }
                        else -> v.toShort()
                    }
                }
                dither = r
            }
            f += n
        }
    }

    /** One block of at most [SUB] frames. False when the samples were left exactly as they were. */
    private fun processBlock(buf: DoubleArray, off: Int, frames: Int): Boolean {
        val ch = channels
        val from = pre
        if (gliding) step()
        val to = if (preDb == 0.0) 1.0 else 10.0.pow(preDb / 20.0)
        pre = to
        var allTransparent = true
        for (s in slots) if (s.active && !s.transparent()) { allTransparent = false; break }
        if (allTransparent && from == 1.0 && to == 1.0) {
            // Nothing changes the samples: keep the history, so a band that moves next starts from the real signal.
            for (c in 0 until ch) {
                val x1: Double; val x2: Double
                if (frames >= 2) { x1 = buf[off + (frames - 1) * ch + c]; x2 = buf[off + (frames - 2) * ch + c] }
                else if (frames == 1) { x1 = buf[off + c]; x2 = hist[c * 2] }
                else continue
                hist[c * 2] = x1; hist[c * 2 + 1] = x2
                for (s in slots) if (s.active) { s.st[c * 4] = x1; s.st[c * 4 + 1] = x2; s.st[c * 4 + 2] = x1; s.st[c * 4 + 3] = x2 }
            }
            return false
        }
        val len = frames * ch
        if (from == to) {
            if (to != 1.0) for (j in 0 until len) buf[off + j] *= to
        } else {
            for (i in 0 until frames) {
                val g = from + (to - from) * (i + 1) / frames
                for (c in 0 until ch) buf[off + i * ch + c] *= g
            }
        }
        for (c in 0 until ch) {
            if (frames >= 2) { hist[c * 2] = buf[off + (frames - 1) * ch + c]; hist[c * 2 + 1] = buf[off + (frames - 2) * ch + c] }
            else if (frames == 1) { hist[c * 2 + 1] = hist[c * 2]; hist[c * 2] = buf[off + c] }
        }
        for (s in slots) {
            if (!s.active) continue
            val b0 = s.b0; val b1 = s.b1; val b2 = s.b2; val a1 = s.a1; val a2 = s.a2
            val st = s.st
            for (c in 0 until ch) {
                var x1 = st[c * 4]; var x2 = st[c * 4 + 1]; var y1 = st[c * 4 + 2]; var y2 = st[c * 4 + 3]
                var i = off + c
                val end = off + len
                while (i < end) {
                    val x = buf[i]
                    // Each delay's feedforward and feedback paired: exactly x at 0 dB.
                    val y = b0 * x + (b1 * x1 - a1 * y1) + (b2 * x2 - a2 * y2)
                    x2 = x1; x1 = x; y2 = y1; y1 = y
                    buf[i] = y
                    i += ch
                }
                // No denormals in silence.
                if (abs(x1) < 1e-30) x1 = 0.0
                if (abs(x2) < 1e-30) x2 = 0.0
                if (abs(y1) < 1e-30) y1 = 0.0
                if (abs(y2) < 1e-30) y2 = 0.0
                st[c * 4] = x1; st[c * 4 + 1] = x2; st[c * 4 + 2] = y1; st[c * 4 + 3] = y2
            }
        }
        return true
    }

    companion object {
        /** Frames between coefficient updates while a band glides. */
        const val SUB = 32
        /** Time constant of the glide to a new setting. */
        const val GLIDE_SECS = 0.02
        const val MAX_CHANNELS = 8
    }
}
