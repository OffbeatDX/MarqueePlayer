package com.offbeat.marquee.player.playback

import android.content.Context
import com.offbeat.marquee.player.diag.AppLog
import androidx.annotation.OptIn
import androidx.media3.common.Format
import androidx.media3.common.MimeTypes
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.DefaultRenderersFactory
import androidx.media3.exoplayer.audio.AudioSink
import androidx.media3.exoplayer.audio.DefaultAudioSink
import androidx.media3.exoplayer.audio.ForwardingAudioSink
import java.nio.ByteBuffer
import java.util.concurrent.atomic.AtomicBoolean

/**
 * The player's audio sink: Media3's [DefaultAudioSink] with the [EqAudioProcessor] in its chain, and one
 * addition: it can configure itself again with the format it was last given, which is how the
 * equalizer becomes active or inactive in the middle of a track (Media3 decides which processors run
 * only when the sink configures). It does that on the playback thread, between two input buffers.
 *
 * What that does in Media3 1.5.1: the new configuration is pending; the sink drains the processors
 * (what they hold comes out), then, the output format being the same (the equalizer keeps 16-bit),
 * swaps the processor chain on the same AudioTrack: no new AudioTrack, no gap, the queue's gapless
 * state untouched. The format is passed again without its encoder delay, so the trimming processor
 * doesn't cut the start of a track from its middle; for MP3 or AAC the encoder padding it was holding
 * back at that moment (up to about 25 ms) is dropped once. FLAC and Opus have none.
 */
@OptIn(UnstableApi::class)
class EqAudioSink(sink: AudioSink) : ForwardingAudioSink(sink) {
    private val pending = AtomicBoolean(false)
    private var lastFormat: Format? = null
    private var lastBufferSize = 0
    private var lastChannels: IntArray? = null
    /** The renderer is still handing over the same buffer (the sink took only part of it). */
    private var midBuffer = false

    fun requestReconfigure() = pending.set(true)

    override fun configure(inputFormat: Format, specifiedBufferSize: Int, outputChannels: IntArray?) {
        if (inputFormat.sampleMimeType == MimeTypes.AUDIO_RAW) {
            lastFormat = inputFormat
            lastBufferSize = specifiedBufferSize
            lastChannels = outputChannels
        } else {
            lastFormat = null // offloaded: no processors run
        }
        pending.set(false)
        super.configure(inputFormat, specifiedBufferSize, outputChannels)
    }

    override fun handleBuffer(buffer: ByteBuffer, presentationTimeUs: Long, encodedAccessUnitCount: Int): Boolean {
        if (!midBuffer && pending.getAndSet(false)) {
            lastFormat?.let { f ->
                AppLog.i(TAG, "equalizer switched: the sink configures again (${f.sampleRate} Hz x ${f.channelCount})")
                super.configure(f.buildUpon().setEncoderDelay(0).build(), lastBufferSize, lastChannels)
            }
        }
        val consumed = super.handleBuffer(buffer, presentationTimeUs, encodedAccessUnitCount)
        midBuffer = !consumed
        return consumed
    }

    override fun flush() {
        midBuffer = false
        super.flush()
    }

    override fun reset() {
        midBuffer = false
        super.reset()
    }

    /** The renderers, with this sink for audio. */
    class Renderers(context: Context, private val eq: EqAudioProcessor, private val onSink: (EqAudioSink) -> Unit) : DefaultRenderersFactory(context) {
        override fun buildAudioSink(context: Context, enableFloatOutput: Boolean, enableAudioTrackPlaybackParams: Boolean): AudioSink {
            val sink = DefaultAudioSink.Builder(context)
                // Float output would make Media3 skip custom processors for high-resolution input.
                .setEnableFloatOutput(false)
                .setEnableAudioTrackPlaybackParams(enableAudioTrackPlaybackParams)
                .setAudioProcessorChain(DefaultAudioSink.DefaultAudioProcessorChain(eq))
                .build()
            return EqAudioSink(sink).also(onSink)
        }
    }

    private companion object {
        const val TAG = "MarqueeEq"
    }
}
