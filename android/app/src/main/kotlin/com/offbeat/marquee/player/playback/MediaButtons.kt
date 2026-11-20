package com.offbeat.marquee.player.playback

import android.content.Context
import android.content.Intent
import androidx.annotation.OptIn
import androidx.media3.common.util.UnstableApi
import androidx.media3.session.MediaButtonReceiver

/**
 * Media buttons (Bluetooth, headsets, the car) while the service isn't running. A play starts the
 * service in the foreground only when there is a saved queue to resume: with none,
 * onPlaybackResumption fails, nothing is shown, and Android would end the app for a foreground
 * service that never called startForeground.
 */
@OptIn(UnstableApi::class)
class MediaButtons : MediaButtonReceiver() {
    override fun shouldStartForegroundService(intent: Intent): Boolean {
        val store = queue ?: return true
        return store.load() != null
    }

    companion object {
        @Volatile
        private var queue: QueueStore? = null

        fun init(context: Context) {
            queue = QueueStore(context)
        }
    }
}
