package com.offbeat.marquee.player.playback

import android.os.Bundle
import androidx.media3.session.SessionCommand

/** The custom session commands and session extras shared by the service and its controllers. */
object PlaybackCommands {
    const val FAVOURITE = "marquee.FAVOURITE"

    /** Args: [ARG_MINUTES] > 0 for minutes, [SLEEP_OFF] or [SLEEP_END_OF_TRACK]. */
    const val SLEEP_TIMER = "marquee.SLEEP_TIMER"
    const val ARG_MINUTES = "minutes"
    const val SLEEP_OFF = 0
    const val SLEEP_END_OF_TRACK = -1

    /**
     * Queue edits, done in the service so the play order stays right under shuffle. Positions are
     * in play order (what the queue sheet shows); [ARG_INDEX] is a timeline index.
     */
    const val ENQUEUE = "marquee.ENQUEUE" // ARG_SONGS (JSON list of songs), ARG_NEXT
    const val QUEUE_MOVE = "marquee.QUEUE_MOVE" // ARG_FROM, ARG_TO
    const val QUEUE_REMOVE = "marquee.QUEUE_REMOVE" // ARG_INDEX
    const val QUEUE_CLEAR_UPCOMING = "marquee.QUEUE_CLEAR_UPCOMING"
    const val ARG_SONGS = "songs"
    const val ARG_NEXT = "next"
    const val ARG_FROM = "from"
    const val ARG_TO = "to"
    const val ARG_INDEX = "index"

    /** Service to controllers: a short line for the snackbar ([ARG_TEXT]). */
    const val NOTICE = "marquee.NOTICE"
    const val ARG_TEXT = "text"

    // Session extras: the sleep timer's state for controllers.
    const val EXTRA_SLEEP_MODE = "marquee.sleep.mode" // "off", "at", "track"
    const val EXTRA_SLEEP_ENDS_AT_WALL = "marquee.sleep.endsAt" // System.currentTimeMillis() at the end
    // The current song's favourite state (the notification's heart shows the same).
    const val EXTRA_FAVOURITE = "marquee.favourite"
    // A seek waiting for the track to load (a live transcode can't seek until it is whole on the
    // phone): the position it will go to, in ms; absent otherwise.
    const val EXTRA_PENDING_SEEK_MS = "marquee.seek.pendingMs"

    val favourite = SessionCommand(FAVOURITE, Bundle.EMPTY)
    val sleepTimer = SessionCommand(SLEEP_TIMER, Bundle.EMPTY)
    val enqueue = SessionCommand(ENQUEUE, Bundle.EMPTY)
    val queueMove = SessionCommand(QUEUE_MOVE, Bundle.EMPTY)
    val queueRemove = SessionCommand(QUEUE_REMOVE, Bundle.EMPTY)
    val queueClearUpcoming = SessionCommand(QUEUE_CLEAR_UPCOMING, Bundle.EMPTY)
    val notice = SessionCommand(NOTICE, Bundle.EMPTY)

    fun sleepArgs(minutes: Int): Bundle = Bundle().apply { putInt(ARG_MINUTES, minutes) }
}
