package com.offbeat.marquee.core.playback

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

sealed interface SleepMode {
    data object Off : SleepMode
    /** Pause at [endsAtMs] (on the timer's clock), after fading out over the last [SleepTimer.FADE_MS]. */
    data class At(val endsAtMs: Long) : SleepMode
    /** Pause when the current track ends. The player does this itself (pauseAtEndOfMediaItems). */
    data object EndOfTrack : SleepMode
}

/**
 * The sleep timer. With minutes set, it does nothing until [FADE_MS] before the end (one delay, no
 * ticking), then steps the fade every [FADE_STEP_MS] through [onFade] (1.0 → 0.0), then calls
 * [onExpire] (pause) and goes back to off with the fade reset to 1.0.
 */
class SleepTimer(
    private val scope: CoroutineScope,
    private val clock: () -> Long,
    private val onFade: (Float) -> Unit,
    private val onExpire: () -> Unit,
) {
    private val _mode = MutableStateFlow<SleepMode>(SleepMode.Off)
    val mode: StateFlow<SleepMode> = _mode.asStateFlow()
    private var job: Job? = null

    fun setMinutes(minutes: Int) {
        require(minutes > 0)
        cancelJob()
        val endsAt = clock() + minutes * 60_000L
        _mode.value = SleepMode.At(endsAt)
        job = scope.launch {
            val fadeStart = endsAt - FADE_MS
            delay((fadeStart - clock()).coerceAtLeast(0))
            while (true) {
                val now = clock()
                if (now >= endsAt) break
                onFade(fadeFactor(now, endsAt))
                delay(minOf(FADE_STEP_MS, endsAt - now))
            }
            onExpire()
            onFade(1f)
            _mode.value = SleepMode.Off
        }
    }

    fun setEndOfTrack() {
        cancelJob()
        _mode.value = SleepMode.EndOfTrack
    }

    /** Off; also called once the end-of-track pause has happened. */
    fun cancel() {
        cancelJob()
        _mode.value = SleepMode.Off
    }

    private fun cancelJob() {
        if (job?.isActive == true) onFade(1f)
        job?.cancel()
        job = null
    }

    companion object {
        const val FADE_MS = 10_000L
        const val FADE_STEP_MS = 250L

        /** The volume factor at [now]: 1.0 until the fade starts, then linear down to 0.0 at the end. */
        fun fadeFactor(now: Long, endsAt: Long, fadeMs: Long = FADE_MS): Float {
            val left = endsAt - now
            return when {
                left >= fadeMs -> 1f
                left <= 0 -> 0f
                else -> left.toFloat() / fadeMs
            }
        }
    }
}
