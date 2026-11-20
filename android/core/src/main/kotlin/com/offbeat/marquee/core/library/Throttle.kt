package com.offbeat.marquee.core.library

/**
 * Work done "when the app opens" that must not be done again for every activity re-creation
 * (a rotation) or every cold start of the process (the widget, a media button, a WorkManager job):
 * due when it never ran, when [everyMs] has passed since [lastMs], or when the clock went back.
 */
object Throttle {
    /** Pinned playlists checked against the server, at most this often when the app opens. */
    const val PLAYLIST_SYNC_MS = 60 * 60_000L

    /** Missing download covers looked for at start, at most this often (pinning fetches its own at once). */
    const val COVER_SYNC_MS = 24 * 60 * 60_000L

    fun isDue(lastMs: Long, nowMs: Long, everyMs: Long): Boolean =
        lastMs <= 0 || nowMs < lastMs || nowMs - lastMs >= everyMs
}
