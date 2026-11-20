package com.offbeat.marquee.player.playback

import androidx.annotation.OptIn
import androidx.media3.common.C
import androidx.media3.common.ForwardingPlayer
import androidx.media3.common.Player
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.ExoPlayer

/**
 * The player the media session hands out, so every controller goes through it: the app, the
 * notification and lock screen, Bluetooth and headset keys, Android Auto, the widget. It is the
 * ExoPlayer, except that every seek inside the current item goes to [onSeek] first (the service's
 * [com.offbeat.marquee.core.playback.SeekReload]), which says whether it dealt with it (true) or
 * the player should seek as usual (false).
 *
 * A live transcode can't seek: ExoPlayer then withdraws the seek commands (the system's seek bar
 * goes) and reports no duration. While the current item can't seek by itself, this player keeps
 * the seek commands available and reports the song's own duration, so the seek reaches the service,
 * which loads the track again from its stored copy once it is complete.
 */
@OptIn(UnstableApi::class)
class SeekingPlayer(
    private val exo: ExoPlayer,
    private val onSeek: (positionMs: Long) -> Boolean,
) : ForwardingPlayer(exo) {

    /** The current item can't seek by itself (a live stream, or opening and not known yet). */
    private fun needsHelp(): Boolean =
        exo.currentMediaItem != null && !exo.isPlayingAd && !exo.isCurrentMediaItemSeekable

    override fun getAvailableCommands(): Player.Commands {
        val commands = super.getAvailableCommands()
        if (!needsHelp()) return commands
        return commands.buildUpon()
            .addAll(
                Player.COMMAND_SEEK_IN_CURRENT_MEDIA_ITEM,
                Player.COMMAND_SEEK_BACK,
                Player.COMMAND_SEEK_FORWARD,
                Player.COMMAND_SEEK_TO_PREVIOUS,
            )
            .build()
    }

    override fun isCommandAvailable(command: Int): Boolean = availableCommands.contains(command)

    override fun getDuration(): Long = super.getDuration().orSongDuration()

    override fun getContentDuration(): Long = super.getContentDuration().orSongDuration()

    private fun Long.orSongDuration(): Long =
        if (this != C.TIME_UNSET || !needsHelp()) this else exo.currentMediaItem?.mediaMetadata?.durationMs ?: C.TIME_UNSET

    /** A song is never live; a live stream's duration would be hidden from the system's controls. */
    override fun isCurrentMediaItemLive(): Boolean = super.isCurrentMediaItemLive() && !needsHelp()

    override fun isCurrentMediaItemSeekable(): Boolean = super.isCurrentMediaItemSeekable() || needsHelp()

    override fun seekTo(positionMs: Long) {
        if (onSeek(positionMs)) return
        super.seekTo(positionMs)
    }

    override fun seekTo(mediaItemIndex: Int, positionMs: Long) {
        if (mediaItemIndex == exo.currentMediaItemIndex && onSeek(positionMs)) return
        super.seekTo(mediaItemIndex, positionMs)
    }

    override fun seekBack() {
        if (!needsHelp()) return super.seekBack()
        seekTo((exo.currentPosition - exo.seekBackIncrement).coerceAtLeast(0))
    }

    override fun seekForward() {
        if (!needsHelp()) return super.seekForward()
        val target = exo.currentPosition + exo.seekForwardIncrement
        val duration = duration
        seekTo(if (duration != C.TIME_UNSET) target.coerceAtMost(duration) else target)
    }

    /** ExoPlayer skips back a track at once in a live stream; a song restarts first, as usual. */
    override fun seekToPrevious() {
        if (!needsHelp()) return super.seekToPrevious()
        if (exo.hasPreviousMediaItem() && exo.currentPosition <= exo.maxSeekToPreviousPosition) {
            exo.seekToPreviousMediaItem()
        } else {
            seekTo(0)
        }
    }

    // The session passes the argument of onAvailableCommandsChanged on to its controllers, and that
    // is the ExoPlayer's set. So once the ExoPlayer has told everyone (onEvents comes after all the
    // single callbacks, in a later message), this player's own set is announced to the same
    // listeners when it differs. (A listener wrapper would have to forward every callback by hand:
    // Kotlin's `by` doesn't forward Java default methods.)

    private val listeners = mutableListOf<Player.Listener>()

    init {
        exo.addListener(object : Player.Listener {
            override fun onEvents(player: Player, events: Player.Events) {
                if (!events.contains(Player.EVENT_AVAILABLE_COMMANDS_CHANGED)) return
                val own = availableCommands
                if (own == exo.availableCommands) return
                listeners.toList().forEach { it.onAvailableCommandsChanged(own) }
            }
        })
    }

    override fun addListener(listener: Player.Listener) {
        listeners += listener
        super.addListener(listener)
    }

    override fun removeListener(listener: Player.Listener) {
        listeners.remove(listener)
        super.removeListener(listener)
    }
}
