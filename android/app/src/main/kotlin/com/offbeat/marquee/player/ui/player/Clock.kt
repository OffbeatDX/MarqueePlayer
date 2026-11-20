package com.offbeat.marquee.player.ui.player

import android.os.SystemClock
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.State
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.withFrameMillis
import com.offbeat.marquee.player.playback.NowPlaying
import kotlinx.coroutines.delay

/**
 * The time the playback position is drawn at. It moves only while playing, in steps of [stepMs]
 * (about one pixel of the bar), and only while frames are being drawn: each step waits for a frame
 * first, and the frame clock stops when the activity is stopped. The player is never asked.
 * Read it in a draw or layout lambda where possible, so a step redraws without recomposing.
 * With [enabled] false (its view is covered) it stands still.
 */
@Composable
fun rememberPlaybackClock(np: NowPlaying, stepMs: Long, enabled: Boolean = true): State<Long> {
    val now = remember { mutableLongStateOf(SystemClock.elapsedRealtime()) }
    LaunchedEffect(np.isPlaying, np.snapshotAtElapsedMs, stepMs, enabled) {
        now.longValue = SystemClock.elapsedRealtime()
        if (!np.isPlaying || !enabled) return@LaunchedEffect
        while (true) {
            withFrameMillis { } // suspends while nothing is drawn
            now.longValue = SystemClock.elapsedRealtime()
            delay(stepMs)
        }
    }
    return now
}

/** A step of about one pixel on a bar [barPx] wide, between 50 ms and a second. */
fun stepFor(durationMs: Long, barPx: Int): Long =
    if (durationMs <= 0 || barPx <= 0) 1000L else (durationMs / barPx).coerceIn(50L, 1000L)
