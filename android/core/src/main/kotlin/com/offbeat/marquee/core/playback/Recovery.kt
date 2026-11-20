package com.offbeat.marquee.core.playback

import com.offbeat.marquee.core.sync.Backoff

/**
 * What to do when the player fails on a track. A network error is retried (after [backoff], or as
 * soon as the network is back when there is none); the [maxFailures]th failure in a row of the same
 * track skips it, as does any other kind of error (a missing file, a format the phone can't
 * decode). Playing again resets the count.
 */
class PlaybackRecovery(private val maxFailures: Int = 3, private val backoff: Backoff = Backoff.PLAYBACK) {
    sealed interface Action {
        data class Retry(val afterMs: Long, val failures: Int) : Action
        data class WaitForNetwork(val failures: Int) : Action
        data object Skip : Action
    }

    private var track: String? = null
    private var failures = 0

    fun onError(trackKey: String, networkError: Boolean, online: Boolean): Action {
        if (trackKey != track) {
            track = trackKey
            failures = 0
        }
        failures++
        if (!networkError || failures >= maxFailures) {
            reset()
            return Action.Skip
        }
        return if (online) Action.Retry(backoff.delayFor(failures), failures) else Action.WaitForNetwork(failures)
    }

    /** The track is playing: its failures are forgotten. */
    fun onPlaying(trackKey: String) {
        if (trackKey == track) reset()
    }

    fun reset() {
        track = null
        failures = 0
    }
}

/**
 * The buffering watchdog. The player can sit in BUFFERING with every byte it needs already on the
 * phone: its own HTTP read is blocked on a connection that died when the network changed, and
 * CacheDataSource only moves back to the cache after that read returns. When buffering has lasted
 * [afterMs] while wanting to play and the whole track is stored, playback is re-prepared at its
 * position (at most [maxPerTrack] times per track).
 */
object StallRule {
    const val AFTER_MS = 8_000L
    const val MAX_PER_TRACK = 2

    fun shouldRecover(bufferingForMs: Long, playWhenReady: Boolean, trackStored: Boolean, recoveriesThisTrack: Int): Boolean =
        playWhenReady && trackStored && bufferingForMs >= AFTER_MS && recoveriesThisTrack < MAX_PER_TRACK
}
