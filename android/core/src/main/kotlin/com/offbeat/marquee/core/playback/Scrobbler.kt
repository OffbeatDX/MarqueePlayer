package com.offbeat.marquee.core.playback

import com.offbeat.marquee.core.subsonic.SubsonicError
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * When a play counts: after 75% of the track or 4 minutes of listening, whichever comes first (the
 * desktop's rule). Listening time is time actually played; seeking doesn't count.
 * Tracks of 30 seconds or less are never submitted.
 */
object ScrobbleRule {
    const val FRACTION = 0.75
    const val MAX_MS = 4 * 60_000L
    const val MIN_TRACK_MS = 30_000L

    /** Listening time after which [durationMs] counts as played; null when it never does. */
    fun thresholdMs(durationMs: Long?): Long? {
        if (durationMs == null || durationMs <= 0) return MAX_MS
        if (durationMs <= MIN_TRACK_MS) return null
        return minOf((durationMs * FRACTION).toLong(), MAX_MS)
    }

    fun shouldSubmit(playedMs: Long, durationMs: Long?): Boolean {
        val t = thresholdMs(durationMs) ?: return false
        return playedMs >= t
    }
}

/**
 * "Now playing" when a track starts, and one submission once it has been listened to long enough
 * ([ScrobbleRule]). Everything that sends lives here.
 *
 * [send] is `SubsonicClient.scrobble`; in read-only builds it throws
 * [SubsonicError.RefusedReadOnly] before anything leaves the device, and that is only logged.
 *
 * With [queue] set (the app), a submission is handed to the durable outbox
 * ([com.offbeat.marquee.core.sync.Outbox]) with its play time instead of being sent, so plays made
 * offline are sent later, batched with the network already up. "Now playing" is never queued:
 * [send] is tried once, and dropped when it fails.
 */
class Scrobbler(
    private val scope: CoroutineScope,
    private val clock: () -> Long,
    private val send: suspend (id: String, timeMillis: Long, submission: Boolean) -> Unit,
    private val enabled: () -> Boolean = { true },
    private val log: (String) -> Unit = {},
    private val queue: ((id: String, timeMillis: Long) -> Unit)? = null,
) {
    private var trackKey: String? = null
    private var songId: String? = null
    private var durationMs: Long? = null
    private var startedAtWallMs = 0L
    private var playedMs = 0L
    private var playingSince: Long? = null
    private var submitted = false
    private var timer: Job? = null

    /** The current queue entry repeated (repeat-one): the next [onItem] starts a fresh play. */
    fun onRepeat() {
        trackKey = null
    }

    /**
     * Follows the player's current queue entry. [trackKey] identifies the queue entry (not just
     * the song id), so the same song twice in a row in the queue counts as two plays. "Now playing"
     * waits for [playWhenReady], so a queue restored paused sends nothing.
     */
    fun onItem(
        trackKey: String?,
        songId: String?,
        durationMs: Long?,
        playWhenReady: Boolean,
        isPlaying: Boolean,
        wallClockMs: Long,
    ) {
        if (trackKey == null || songId == null) {
            onStopped()
            return
        }
        if (trackKey != this.trackKey && playWhenReady) {
            this.trackKey = trackKey
            onTrackStarted(songId, durationMs, isPlaying, wallClockMs)
        } else {
            onPlayingChanged(isPlaying)
        }
    }

    /** A new track became current. Settles the previous play first. [playing] says whether it is playing right now. */
    fun onTrackStarted(id: String, durationMs: Long?, playing: Boolean, wallClockMs: Long) {
        onPlayingChanged(false)
        timer?.cancel()
        songId = id
        this.durationMs = durationMs
        startedAtWallMs = wallClockMs
        playedMs = 0
        submitted = false
        playingSince = if (playing) clock() else null
        if (enabled()) launchSend(id, wallClockMs, submission = false)
        if (playing) arm()
    }

    fun onPlayingChanged(playing: Boolean) {
        if (songId == null) return
        val now = clock()
        if (playing) {
            if (playingSince == null) playingSince = now
            arm()
        } else {
            playingSince?.let { playedMs += now - it }
            playingSince = null
            timer?.cancel()
            maybeSubmit()
        }
    }

    /** Nothing is current any more (queue cleared, service stopping). */
    fun onStopped() {
        onPlayingChanged(false)
        timer?.cancel()
        songId = null
        trackKey = null
    }

    /** Listening time so far on the current track. */
    fun playedMs(): Long = playedMs + (playingSince?.let { clock() - it } ?: 0)

    private fun arm() {
        timer?.cancel()
        if (submitted) return
        val threshold = ScrobbleRule.thresholdMs(durationMs) ?: return
        val wait = (threshold - playedMs()).coerceAtLeast(0)
        timer = scope.launch {
            delay(wait)
            maybeSubmit()
        }
    }

    private fun maybeSubmit() {
        val id = songId ?: return
        if (submitted || !ScrobbleRule.shouldSubmit(playedMs(), durationMs)) return
        submitted = true
        if (!enabled()) return
        val q = queue
        if (q != null) {
            q(id, startedAtWallMs)
            log("scrobble submission queued for $id")
        } else {
            launchSend(id, startedAtWallMs, submission = true)
        }
    }

    private fun launchSend(id: String, time: Long, submission: Boolean) {
        scope.launch {
            val what = if (submission) "submission" else "now playing"
            try {
                send(id, time, submission)
                log("scrobble $what sent for $id")
            } catch (e: SubsonicError.RefusedReadOnly) {
                log("scrobble $what refused (read-only build), not sent")
            } catch (e: SubsonicError) {
                log("scrobble $what failed: ${e.message}")
            }
        }
    }
}
