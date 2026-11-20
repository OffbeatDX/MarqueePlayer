package com.offbeat.marquee.player.playback

import com.offbeat.marquee.player.diag.AppLog
import androidx.annotation.OptIn
import androidx.media3.common.Player
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.analytics.AnalyticsListener
import androidx.media3.exoplayer.source.LoadEventInfo
import androidx.media3.exoplayer.source.MediaLoadData
import java.io.IOException

/**
 * Debug builds: the player's loads and states, for checking prefetch and buffering on the
 * emulator. Never logs a URI (they carry the token): only sizes, times and states.
 */
@OptIn(UnstableApi::class)
class DebugLoadLogger : AnalyticsListener {
    override fun onLoadStarted(t: AnalyticsListener.EventTime, info: LoadEventInfo, data: MediaLoadData) {
        AppLog.d(TAG, "load started (item ${t.windowIndex})")
    }

    override fun onLoadCompleted(t: AnalyticsListener.EventTime, info: LoadEventInfo, data: MediaLoadData) {
        AppLog.d(TAG, "load completed (item ${t.windowIndex}): ${info.bytesLoaded} bytes in ${info.loadDurationMs} ms")
    }

    override fun onLoadCanceled(t: AnalyticsListener.EventTime, info: LoadEventInfo, data: MediaLoadData) {
        AppLog.d(TAG, "load canceled (item ${t.windowIndex}): ${info.bytesLoaded} bytes in ${info.loadDurationMs} ms")
    }

    override fun onLoadError(t: AnalyticsListener.EventTime, info: LoadEventInfo, data: MediaLoadData, error: IOException, wasCanceled: Boolean) {
        AppLog.d(TAG, "load error (item ${t.windowIndex}): ${error.javaClass.simpleName} after ${info.bytesLoaded} bytes")
    }

    override fun onIsLoadingChanged(t: AnalyticsListener.EventTime, isLoading: Boolean) {
        AppLog.d(TAG, "loading=$isLoading buffered=${t.totalBufferedDurationMs} ms")
    }

    override fun onPlaybackStateChanged(t: AnalyticsListener.EventTime, state: Int) {
        val name = when (state) {
            Player.STATE_IDLE -> "idle"
            Player.STATE_BUFFERING -> "buffering"
            Player.STATE_READY -> "ready"
            else -> "ended"
        }
        AppLog.d(TAG, "state $name at ${t.currentPlaybackPositionMs} ms, buffered ${t.totalBufferedDurationMs} ms")
    }

    override fun onAudioInputFormatChanged(t: AnalyticsListener.EventTime, format: androidx.media3.common.Format, decoderReuseEvaluation: androidx.media3.exoplayer.DecoderReuseEvaluation?) {
        AppLog.d(TAG, "audio format (item ${t.windowIndex}): ${format.sampleMimeType} ${format.sampleRate} Hz x ${format.channelCount}")
    }

    private companion object {
        const val TAG = "MarqueeLoad"
    }
}
