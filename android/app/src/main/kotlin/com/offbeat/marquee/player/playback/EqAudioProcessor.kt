package com.offbeat.marquee.player.playback

import android.os.SystemClock
import com.offbeat.marquee.player.diag.AppLog
import androidx.annotation.OptIn
import androidx.media3.common.C
import androidx.media3.common.audio.AudioProcessor.AudioFormat
import androidx.media3.common.audio.BaseAudioProcessor
import androidx.media3.common.util.UnstableApi
import com.offbeat.marquee.core.eq.EqParams
import com.offbeat.marquee.core.eq.Equalizer
import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * The built-in equalizer as a Media3 [androidx.media3.common.audio.AudioProcessor] in
 * [androidx.media3.exoplayer.audio.DefaultAudioSink]'s chain. 16-bit PCM in (what Media3 1.5's sink
 * hands its processors: it converts everything to 16-bit first unless float output is on, and then
 * it skips custom processors for high-resolution input), converted to double, equalized with the
 * automatic preamp as headroom, and back to 16-bit with TPDF dither. The output format is the input's,
 * so switching the equalizer on or off never changes the AudioTrack's format: the sink swaps the
 * processor chain on the same AudioTrack, with no gap.
 *
 * **Active only while on.** [onConfigure] makes it inactive (`isActive` false, no CPU at all) while
 * the equalizer is off. Whether it is active is decided when the sink configures, so a switch asks
 * the sink to configure again ([onReconfigureNeeded], see [EqAudioSink]): switched on, at once, and the
 * bands glide in from flat; switched off, once the bands have glided out to flat, so neither way steps.
 *
 * Filter state: emptied on a seek or a new item (a flush with no end of stream before it); kept across
 * a gapless boundary in the same format (end of stream, then a flush), so nothing rings or clicks
 * there; worked out again for another sample rate or channel count.
 *
 * Threads: [update] from the main thread (a volatile hand-over); everything else on the playback thread.
 */
@OptIn(UnstableApi::class)
class EqAudioProcessor : BaseAudioProcessor() {
    /** The latest setting from the screen, taken up at the next buffer. */
    @Volatile private var latest: EqParams = EqParams.OFF
    /** Called (from any thread) when the sink should configure again so the active state can change. */
    @Volatile var onReconfigureNeeded: (() -> Unit)? = null
    /** Flushed as active: read by [update] on the main thread. */
    @Volatile var isRunning: Boolean = false
        private set
    @Volatile private var switchedOn = false

    private var eq: Equalizer? = null
    private var applied: EqParams? = null
    private var endedSinceInput = false
    private var askedToStop = false
    private var src = ShortArray(0)
    private var dst = ShortArray(0)
    // A one-off measurement of what it costs, logged after the first 10 s of audio it equalizes.
    private var costNs = 0L
    private var costFrames = 0L
    private var costLogged = false

    fun update(params: EqParams) {
        val wasOn = latest.enabled
        latest = params
        if (params.enabled && !wasOn) {
            switchedOn = true
            if (!isRunning) onReconfigureNeeded?.invoke()
        }
        // Switched off: it glides out first; queueInput asks for the reconfiguration once flat.
    }

    override fun onConfigure(inputAudioFormat: AudioFormat): AudioFormat {
        if (!latest.enabled) return AudioFormat.NOT_SET
        if (inputAudioFormat.encoding != C.ENCODING_PCM_16BIT || inputAudioFormat.channelCount !in 1..Equalizer.MAX_CHANNELS) {
            AppLog.w(TAG, "equalizer can't take $inputAudioFormat: left out")
            return AudioFormat.NOT_SET
        }
        return inputAudioFormat
    }

    override fun onFlush() {
        val active = outputAudioFormat != AudioFormat.NOT_SET
        val wasRunning = isRunning
        isRunning = active
        askedToStop = false
        if (!active) {
            if (wasRunning) AppLog.i(TAG, "equalizer processor inactive")
            endedSinceInput = false
            switchedOn = false
            return
        }
        val rate = inputAudioFormat.sampleRate
        val ch = inputAudioFormat.channelCount
        val p = latest
        var e = eq
        when {
            e == null || e.channels != ch || e.rate != rate -> {
                if (e == null) e = Equalizer(rate, ch).also { eq = it } else {
                    AppLog.i(TAG, "equalizer format: ${e.rate} Hz x ${e.channels} -> $rate Hz x $ch, worked out again")
                    e.setFormat(rate, ch)
                }
                if (switchedOn && !wasRunning) e.glideInTo(p) else { e.setParams(p); e.reset() }
            }
            !wasRunning -> if (switchedOn) e.glideInTo(p) else { e.setParams(p); e.reset() }
            // A gapless boundary in the same format: the filters ring on into the next track.
            endedSinceInput -> e.setParams(p)
            else -> { e.setParams(p); e.reset() }
        }
        applied = p
        switchedOn = false
        endedSinceInput = false
        if (!wasRunning) {
            AppLog.i(TAG, "equalizer processor active: $rate Hz x $ch, preamp ${"%.2f".format(p.preampDb(rate) + 0.0)} dB")
            costNs = 0; costFrames = 0; costLogged = false
        }
    }

    override fun queueInput(inputBuffer: ByteBuffer) {
        val bytes = inputBuffer.remaining()
        if (bytes == 0) return
        endedSinceInput = false
        val e = eq
        val out = replaceOutputBuffer(bytes)
        if (e == null) {
            out.put(inputBuffer).flip()
            return
        }
        val p = latest
        if (p !== applied) {
            applied = p
            e.setParams(p)
        }
        val samples = bytes / 2
        if (src.size < samples) { src = ShortArray(samples); dst = ShortArray(samples) }
        inputBuffer.order(ByteOrder.nativeOrder()).asShortBuffer().get(src, 0, samples)
        inputBuffer.position(inputBuffer.position() + samples * 2)
        val frames = samples / e.channels
        val t0 = if (costLogged) 0L else SystemClock.elapsedRealtimeNanos()
        e.processPcm16(src, 0, dst, 0, frames)
        if (!costLogged) logCost(SystemClock.elapsedRealtimeNanos() - t0, frames, e)
        out.asShortBuffer().put(dst, 0, samples)
        out.position(samples * 2)
        out.flip()
        if (!p.enabled && e.isBypassed && !askedToStop) {
            askedToStop = true
            AppLog.i(TAG, "equalizer glided out to flat: asking the sink to drop it")
            onReconfigureNeeded?.invoke()
        }
    }

    private fun logCost(ns: Long, frames: Int, e: Equalizer) {
        if (e.isBypassed) return
        costNs += ns
        costFrames += frames
        if (costFrames >= e.rate * 10L) {
            costLogged = true
            AppLog.i(TAG, "equalizer cost: %.1f ns per frame over %.1f s at %d Hz x %d (%d bands), %d samples clamped".format(
                costNs.toDouble() / costFrames, costFrames.toDouble() / e.rate, e.rate, e.channels, e.params.bands.size, e.clipped,
            ))
        }
    }

    override fun onQueueEndOfStream() {
        endedSinceInput = true
    }

    override fun onReset() {
        eq = null
        applied = null
        isRunning = false
        endedSinceInput = false
        src = ShortArray(0)
        dst = ShortArray(0)
    }

    private companion object {
        const val TAG = "MarqueeEq"
    }
}
