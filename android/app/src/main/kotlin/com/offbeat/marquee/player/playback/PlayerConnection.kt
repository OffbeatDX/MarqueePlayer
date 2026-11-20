package com.offbeat.marquee.player.playback

import android.content.ComponentName
import android.content.Context
import android.os.Bundle
import android.os.SystemClock
import androidx.compose.runtime.Immutable
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.Player
import androidx.media3.session.MediaController
import androidx.media3.session.SessionError
import androidx.media3.session.SessionResult
import androidx.media3.session.SessionToken
import androidx.media3.session.SessionCommand
import com.google.common.util.concurrent.Futures
import com.offbeat.marquee.core.offline.Playable
import com.offbeat.marquee.player.art.Covers
import com.offbeat.marquee.player.data.Library
import com.offbeat.marquee.player.net.NetworkMonitor
import com.offbeat.marquee.player.offline.Availability
import com.google.common.util.concurrent.ListenableFuture
import com.google.common.util.concurrent.MoreExecutors
import com.offbeat.marquee.core.subsonic.Song
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlin.coroutines.resume

/**
 * What the mini player and Now Playing draw. The position is a snapshot ([positionMs] at
 * [snapshotAtElapsedMs], moving at [speed] while [isPlaying]); the UI animates from it and never
 * asks the player for the position.
 */
@Immutable
data class NowPlaying(
    val hasItem: Boolean = false,
    val songId: String = "",
    val title: String = "",
    val artist: String = "",
    val artistId: String? = null,
    val album: String = "",
    val albumId: String? = null,
    val coverArt: String? = null,
    val isPlaying: Boolean = false,
    val buffering: Boolean = false,
    val positionMs: Long = 0,
    val snapshotAtElapsedMs: Long = 0,
    val speed: Float = 1f,
    val durationMs: Long = 0,
    val hasNext: Boolean = false,
    val hasPrevious: Boolean = false,
    val shuffle: Boolean = false,
    val repeatMode: Int = Player.REPEAT_MODE_OFF,
    val favourite: Boolean = false,
    val index: Int = 0,
    val sleep: SleepState = SleepState.Off,
    /**
     * A seek waits for the track to load (a live transcode, see the service): [positionMs] is where
     * it will go, held still until it does.
     */
    val seekPending: Boolean = false,
) {
    /** Play or buffering-to-play: the button shows Pause. */
    val playingOrWill: Boolean get() = isPlaying || buffering

    fun positionAt(elapsedMs: Long): Long {
        val p = if (isPlaying && !seekPending) positionMs + ((elapsedMs - snapshotAtElapsedMs) * speed).toLong() else positionMs
        return if (durationMs > 0) p.coerceIn(0, durationMs) else p.coerceAtLeast(0)
    }
}

sealed interface SleepState {
    data object Off : SleepState
    data object EndOfTrack : SleepState
    data class At(val endsAtWallMs: Long) : SleepState
}

/**
 * One row of the queue sheet, in play order: [position] is its place in that order (what queue
 * moves take), [index] its timeline index (what playing and removing take), [uid] tells two
 * entries of the same song apart.
 */
@Immutable
data class QueueRow(
    val position: Int,
    val index: Int,
    val uid: Long,
    val song: Song,
    val title: String,
    val artist: String,
    val coverArt: String?,
    val durationMs: Long,
) {
    val songId: String get() = song.id
}

enum class FavouriteResult { Done, ReadOnly, Failed }

/**
 * The UI's MediaController to [PlaybackService], connected while the activity is started. All
 * playback goes through it. The queue is snapshotted only when the timeline changes.
 */
class PlayerConnection(private val context: Context) {
    private var future: ListenableFuture<MediaController>? = null
    private var controller: MediaController? = null
    private val _state = MutableStateFlow(NowPlaying())
    val state: StateFlow<NowPlaying> = _state.asStateFlow()
    private val _queue = MutableStateFlow<List<QueueRow>>(emptyList())
    val queue: StateFlow<List<QueueRow>> = _queue.asStateFlow()

    /** Called before a new queue starts (the activity asks for the notification permission once). */
    var onBeforePlay: (() -> Unit)? = null

    private val listener = object : Player.Listener {
        override fun onEvents(player: Player, events: Player.Events) {
            if (events.containsAny(Player.EVENT_TIMELINE_CHANGED, Player.EVENT_SHUFFLE_MODE_ENABLED_CHANGED)) snapshotQueue()
            snapshot()
        }
    }

    private val controllerListener = object : MediaController.Listener {
        override fun onExtrasChanged(controller: MediaController, extras: Bundle) = snapshot()

        override fun onCustomCommand(controller: MediaController, command: SessionCommand, args: Bundle): ListenableFuture<SessionResult> {
            if (command.customAction == PlaybackCommands.NOTICE) args.getString(PlaybackCommands.ARG_TEXT)?.let(Library::post)
            return Futures.immediateFuture(SessionResult(SessionResult.RESULT_SUCCESS))
        }
    }

    fun connect() {
        if (future != null) return
        val token = SessionToken(context, ComponentName(context, PlaybackService::class.java))
        val f = MediaController.Builder(context, token).setListener(controllerListener).buildAsync()
        future = f
        f.addListener({
            val c = runCatching { f.get() }.getOrNull() ?: return@addListener
            if (future !== f) {
                c.release()
                return@addListener
            }
            controller = c
            c.addListener(listener)
            snapshotQueue()
            snapshot()
        }, MoreExecutors.directExecutor())
    }

    fun disconnect() {
        controller?.removeListener(listener)
        future?.let { MediaController.releaseFuture(it) }
        future = null
        controller = null
    }

    /** Plays [songs] from [startIndex]; [shuffled] plays them in a random order (shuffle mode stays off). */
    fun playSongs(songs: List<Song>, startIndex: Int = 0, shuffled: Boolean = false) {
        val c = controller ?: return
        if (songs.isEmpty()) return
        var all = songs
        var start = startIndex
        if (!NetworkMonitor.online) {
            // Offline: what is downloaded or cached plays, the rest is left out.
            val tapped = songs.getOrNull(startIndex)
            if (!shuffled && startIndex > 0 && tapped != null && !Availability.canPlayOffline(tapped.id)) {
                Library.post("Not available offline")
                return
            }
            val q = Playable.queueFrom(songs, startIndex) { Availability.canPlayOffline(it.id) }
            if (q == null) {
                Library.post("Nothing here is available offline")
                return
            }
            all = q.first
            start = q.second
        }
        onBeforePlay?.invoke()
        val list = if (shuffled) all.shuffled() else all
        c.shuffleModeEnabled = false
        c.setMediaItems(list.map(SongItems::toMediaItem), if (shuffled) 0 else start.coerceIn(0, list.lastIndex), 0)
        c.prepare()
        c.play()
    }

    /**
     * "Play next" (right after the current song, in play order) or "Add to queue" (last). The
     * service inserts them so shuffle keeps them where they were put.
     */
    fun enqueue(all: List<Song>, next: Boolean) {
        val c = controller ?: return
        val songs = if (NetworkMonitor.online) all else all.filter { Availability.canPlayOffline(it.id) }
        if (songs.isEmpty()) {
            if (all.isNotEmpty()) Library.post("Nothing here is available offline")
            return
        }
        if (c.mediaItemCount == 0) onBeforePlay?.invoke()
        val args = Bundle().apply {
            putString(PlaybackCommands.ARG_SONGS, SongItems.encodeSongs(songs))
            putBoolean(PlaybackCommands.ARG_NEXT, next)
        }
        c.sendCustomCommand(PlaybackCommands.enqueue, args)
    }

    /** Moves a queue entry from play position [from] to [to]. */
    fun moveInQueue(from: Int, to: Int) {
        val c = controller ?: return
        c.sendCustomCommand(PlaybackCommands.queueMove, Bundle().apply { putInt(PlaybackCommands.ARG_FROM, from); putInt(PlaybackCommands.ARG_TO, to) })
    }

    /** Removes the entry at timeline [index] (never the current one). */
    fun removeFromQueue(index: Int) {
        val c = controller ?: return
        c.sendCustomCommand(PlaybackCommands.queueRemove, Bundle().apply { putInt(PlaybackCommands.ARG_INDEX, index) })
    }

    /** Removes everything after the current song in play order. */
    fun clearUpcoming() {
        val c = controller ?: return
        c.sendCustomCommand(PlaybackCommands.queueClearUpcoming, Bundle.EMPTY)
    }

    fun togglePlay() {
        val c = controller ?: return
        if (c.isPlaying || (c.playWhenReady && c.playbackState == Player.STATE_BUFFERING)) {
            c.pause()
        } else {
            if (c.playbackState == Player.STATE_IDLE) c.prepare()
            c.play()
        }
    }

    fun next() = controller?.seekToNext()
    fun previous() = controller?.seekToPrevious()
    fun seekTo(ms: Long) = controller?.seekTo(ms)

    /** Jumps to a queue entry (by its index in queue order) and plays. */
    fun playAt(index: Int) {
        val c = controller ?: return
        if (index !in 0 until c.mediaItemCount) return
        c.seekToDefaultPosition(index)
        if (c.playbackState == Player.STATE_IDLE) c.prepare()
        c.play()
    }

    fun toggleShuffle() {
        val c = controller ?: return
        c.shuffleModeEnabled = !c.shuffleModeEnabled
    }

    /** Off, all, one, off. */
    fun cycleRepeat() {
        val c = controller ?: return
        c.repeatMode = when (c.repeatMode) {
            Player.REPEAT_MODE_OFF -> Player.REPEAT_MODE_ALL
            Player.REPEAT_MODE_ALL -> Player.REPEAT_MODE_ONE
            else -> Player.REPEAT_MODE_OFF
        }
    }

    fun setSleepTimer(minutes: Int) {
        val c = controller ?: return
        c.sendCustomCommand(PlaybackCommands.sleepTimer, PlaybackCommands.sleepArgs(minutes))
    }

    /** Favourite or unfavourite the current song through the service (which updates the notification's heart). */
    suspend fun toggleFavourite(): FavouriteResult {
        val c = controller ?: return FavouriteResult.Failed
        val f = c.sendCustomCommand(PlaybackCommands.favourite, Bundle.EMPTY)
        val result = suspendCancellableCoroutine<SessionResult?> { cont ->
            f.addListener({ cont.resume(runCatching { f.get() }.getOrNull()) }, MoreExecutors.directExecutor())
        }
        return when (result?.resultCode) {
            SessionResult.RESULT_SUCCESS -> FavouriteResult.Done
            SessionError.ERROR_PERMISSION_DENIED -> FavouriteResult.ReadOnly
            else -> FavouriteResult.Failed
        }
    }

    private fun snapshot() {
        val c = controller ?: return
        val item = c.currentMediaItem
        val extras = c.sessionExtras
        val sleep = when (extras.getString(PlaybackCommands.EXTRA_SLEEP_MODE)) {
            "track" -> SleepState.EndOfTrack
            "at" -> SleepState.At(extras.getLong(PlaybackCommands.EXTRA_SLEEP_ENDS_AT_WALL))
            else -> SleepState.Off
        }
        val song = item?.let { songOf(it) }
        val duration = c.duration.takeIf { it > 0 } ?: item?.mediaMetadata?.durationMs ?: 0
        val pendingSeek = if (extras.containsKey(PlaybackCommands.EXTRA_PENDING_SEEK_MS)) extras.getLong(PlaybackCommands.EXTRA_PENDING_SEEK_MS) else null
        _state.value = NowPlaying(
            hasItem = item != null,
            songId = item?.mediaId.orEmpty(),
            title = item?.mediaMetadata?.title?.toString().orEmpty(),
            artist = item?.mediaMetadata?.artist?.toString().orEmpty(),
            artistId = song?.artistId,
            album = item?.mediaMetadata?.albumTitle?.toString().orEmpty(),
            albumId = song?.albumId,
            coverArt = song?.coverArt,
            isPlaying = c.isPlaying,
            buffering = c.playWhenReady && c.playbackState == Player.STATE_BUFFERING,
            positionMs = pendingSeek ?: c.currentPosition,
            snapshotAtElapsedMs = SystemClock.elapsedRealtime(),
            speed = c.playbackParameters.speed,
            durationMs = duration,
            hasNext = c.hasNextMediaItem(),
            hasPrevious = item != null,
            shuffle = c.shuffleModeEnabled,
            repeatMode = c.repeatMode,
            favourite = if (extras.containsKey(PlaybackCommands.EXTRA_FAVOURITE)) extras.getBoolean(PlaybackCommands.EXTRA_FAVOURITE) else song?.starred != null,
            index = c.currentMediaItemIndex,
            sleep = sleep,
            seekPending = pendingSeek != null,
        )
    }

    /** The current item's song, parsed once per item. */
    private var lastSong: Pair<String, Song?>? = null

    private fun songOf(item: MediaItem): Song? {
        lastSong?.let { (id, s) -> if (id == item.mediaId) return s }
        return SongItems.song(item).also { lastSong = item.mediaId to it }
    }

    /** The queue in play order (the shuffle order while shuffle is on). */
    private fun snapshotQueue() {
        val c = controller ?: return
        val tl = c.currentTimeline
        val shuffle = c.shuffleModeEnabled
        val order = ArrayList<Int>(tl.windowCount)
        if (!tl.isEmpty) {
            var i = tl.getFirstWindowIndex(shuffle)
            while (i != C.INDEX_UNSET && order.size < tl.windowCount) {
                order += i
                i = tl.getNextWindowIndex(i, Player.REPEAT_MODE_OFF, shuffle)
            }
        }
        _queue.value = order.mapIndexedNotNull { position, i ->
            if (i >= c.mediaItemCount) return@mapIndexedNotNull null
            val item = c.getMediaItemAt(i)
            val m = item.mediaMetadata
            QueueRow(
                position = position,
                index = i,
                uid = SongItems.uid(item).takeIf { it != 0L } ?: (i.toLong() shl 32),
                song = SongItems.song(item) ?: Song(item.mediaId),
                title = m.title?.toString().orEmpty(),
                artist = m.artist?.toString().orEmpty(),
                coverArt = SongItems.song(item)?.coverArt ?: Covers.coverIdOf(m.artworkUri),
                durationMs = m.durationMs ?: 0,
            )
        }
    }
}
