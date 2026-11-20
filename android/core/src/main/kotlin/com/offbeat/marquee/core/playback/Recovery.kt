package com.offbeat.marquee.core.playback

import com.offbeat.marquee.core.sync.Backoff

/**
 * What to do when the player fails on a track.
 *
 * A retryable error (network IO, HTTP 5xx, 408 or 429) is retried after [backoff], or once the
 * network is back when offline; the [maxFailures]th failure in a row of the same track gives up on
 * it. An auth error (HTTP 401 or 403) pauses immediately so the user can sign in again. Anything
 * else (HTTP 404, a format the phone can't decode) skips without retrying.
 *
 * When [maxConsecutiveTracks] tracks in a row fail for the same [FailureKind], playback pauses
 * with one notice instead of skipping through the whole queue. Any track that plays resets both
 * counts.
 */
class PlaybackRecovery(
    private val maxFailures: Int = 3,
    private val maxConsecutiveTracks: Int = 3,
    private val backoff: Backoff = Backoff.PLAYBACK,
) {
    /** Why a track failed, classified from its HTTP status (if any) or player error. */
    enum class FailureKind {
        /** Network or transient server error (IO failure, HTTP 5xx, 408, 429): worth retrying. */
        Retryable,
        /** The server refused the sign-in (HTTP 401, 403): stop playback and tell the user. */
        Auth,
        /** Missing on the server (HTTP 404): skip without retrying. */
        NotFound,
        /** Anything else (corrupt file, unsupported format, other HTTP 4xx): skip without retrying. */
        Other,
    }

    sealed interface Action {
        data class Retry(val afterMs: Long, val failures: Int) : Action
        data class WaitForNetwork(val failures: Int) : Action
        data object Skip : Action
        /** Pause playback: either auth failed, or [maxConsecutiveTracks] tracks in a row failed for [kind]. */
        data class Pause(val kind: FailureKind) : Action
    }

    private var track: String? = null
    private var failures = 0
    private var consecutiveKind: FailureKind? = null
    private var consecutiveTracks = 0

    fun onError(trackKey: String, kind: FailureKind, online: Boolean): Action {
        if (kind == FailureKind.Auth) {
            reset()
            return Action.Pause(FailureKind.Auth)
        }
        if (trackKey != track) {
            track = trackKey
            failures = 0
        }
        failures++
        if (kind != FailureKind.Retryable || failures >= maxFailures) {
            clearTrack()
            return recordTrackFailure(kind)
        }
        return if (online) Action.Retry(backoff.delayFor(failures), failures) else Action.WaitForNetwork(failures)
    }

    /** A track is playing: per-track failures and any consecutive-track failure streak are forgotten. */
    fun onPlaying(trackKey: String) {
        reset()
    }

    fun reset() {
        clearTrack()
        consecutiveKind = null
        consecutiveTracks = 0
    }

    private fun clearTrack() {
        track = null
        failures = 0
    }

    private fun recordTrackFailure(kind: FailureKind): Action {
        if (kind == consecutiveKind) {
            consecutiveTracks++
        } else {
            consecutiveKind = kind
            consecutiveTracks = 1
        }
        if (consecutiveTracks >= maxConsecutiveTracks) {
            reset()
            return Action.Pause(kind)
        }
        return Action.Skip
    }

    companion object {
        /**
         * Classifies a playback error from the HTTP status code in its cause chain (when present)
         * and whether the player error code is a network IO error. [errorReply]: the server answered
         * with a Subsonic error in place of the song (HTTP 200, a JSON or XML body), whose
         * [subsonicCode] says why ([StreamReply.kindOf]: a wrong password pauses as Auth).
         */
        fun classify(httpStatus: Int?, networkIo: Boolean, errorReply: Boolean = false, subsonicCode: Int? = null): FailureKind = when {
            errorReply -> StreamReply.kindOf(subsonicCode)
            httpStatus != null -> when (httpStatus) {
                in 500..599, 408, 429 -> FailureKind.Retryable
                401, 403 -> FailureKind.Auth
                404 -> FailureKind.NotFound
                else -> FailureKind.Other
            }
            networkIo -> FailureKind.Retryable
            else -> FailureKind.Other
        }
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
